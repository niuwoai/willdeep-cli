//! Tool-call identity and local-only display projections. Never executes tools.
use super::*;
use narration::{TOOL_DONE, TOOL_FAILED, TOOL_PENDING, tool_line};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ToolVisuals {
    calls: BTreeMap<String, usize>,
    commands: BTreeSet<usize>,
    edits: BTreeSet<usize>,
    dismissed: BTreeSet<usize>,
}

impl ToolVisuals {
    pub(super) fn is_diff(&self, source: usize) -> bool {
        self.edits.contains(&source)
    }

    pub(super) fn automatically_open(&self, source: usize) -> bool {
        !self.dismissed.contains(&source)
            && (self.edits.contains(&source) || self.commands.last() == Some(&source))
    }

    pub(super) fn remap(&mut self, map: impl Fn(usize) -> Option<usize>) {
        self.calls.retain(|_, source| {
            if let Some(next) = map(*source) {
                *source = next;
                true
            } else {
                false
            }
        });
        for set in [&mut self.commands, &mut self.edits, &mut self.dismissed] {
            *set = std::mem::take(set).into_iter().filter_map(&map).collect();
        }
    }

    pub(super) fn finish(&mut self) {
        self.commands.clear();
        self.calls.clear();
    }
}

impl App {
    /// Automatic projections belong to the active turn: once the turn has its
    /// final result, only a row the user opened stays expanded.
    pub(super) fn tool_row_open(&self, source: usize) -> bool {
        self.expanded_tool_rows.contains(&source)
            || (self.running && self.tool_visuals.automatically_open(source))
    }

    pub(super) fn toggle_tool_projection(&mut self, source: usize) {
        if self.tool_row_open(source) {
            self.expanded_tool_rows.remove(&source);
            self.tool_visuals.dismissed.insert(source);
        } else {
            self.tool_visuals.dismissed.remove(&source);
            self.expanded_tool_rows.insert(source);
        }
    }

    pub(super) fn note_tool_call_requested(
        &mut self,
        id: Option<&str>,
        name: &str,
        detail: Option<&str>,
        expansion: Option<String>,
    ) {
        if id.is_some_and(|id| self.tool_visuals.calls.contains_key(id)) {
            return;
        }
        self.finish_thought();
        self.finish_reply_stream();
        self.flush_held_narration();
        self.transient_thought = None;
        self.note_turn_phase(turn_status::TurnPhase::Tool(name.to_owned()));
        self.dismiss_compression_notice();
        self.dismiss_awaiting_reply();
        self.append_transcript(tool_line(TOOL_PENDING, name, detail));
        let source = self.transcript.len().saturating_sub(1);
        if let Some(id) = id {
            self.tool_visuals.calls.insert(id.to_owned(), source);
        }
        if let Some(expansion) = expansion.filter(|text| !text.trim().is_empty()) {
            self.tool_expansions.insert(source, expansion);
            if name == "run_command" {
                self.tool_visuals.commands.insert(source);
            }
        }
        self.transcript_render_cache = None;
        self.refresh_transcript_height();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn note_tool_call_completed(
        &mut self,
        id: Option<&str>,
        name: &str,
        detail: Option<&str>,
        expansion: Option<String>,
        is_error: bool,
        edit_display: Option<&serde_json::Value>,
    ) {
        self.last_progress_at = Some(Instant::now());
        if matches!(&self.turn_phase, turn_status::TurnPhase::Tool(active) if active == name) {
            let phase = self
                .tools
                .details
                .iter()
                .rev()
                .find_map(|detail| {
                    detail
                        .strip_prefix("… ")
                        .map(|name| turn_status::TurnPhase::Tool(name.to_owned()))
                })
                .unwrap_or(turn_status::TurnPhase::Preparing);
            self.note_turn_phase(phase);
        }
        let marker = if is_error { TOOL_FAILED } else { TOOL_DONE };
        let pending = tool_line(TOOL_PENDING, name, None);
        let with_detail = format!("{pending} · ");
        // An identified completion must never consume another same-name call.
        let source = id
            .and_then(|id| self.tool_visuals.calls.get(id).copied())
            .or_else(|| {
                id.is_none()
                    .then(|| {
                        self.transcript
                            .iter()
                            .rposition(|line| *line == pending || line.starts_with(&with_detail))
                    })
                    .flatten()
            });
        let source = if let Some(source) = source.filter(|&index| index < self.transcript.len()) {
            self.transcript[source] = self.transcript[source].replacen(TOOL_PENDING, marker, 1);
            source
        } else {
            self.append_transcript(tool_line(marker, name, detail));
            let source = self.transcript.len().saturating_sub(1);
            if let Some(id) = id {
                self.tool_visuals.calls.insert(id.to_owned(), source);
            }
            source
        };
        self.tool_visuals.commands.remove(&source);
        if let Some(expansion) = expansion.filter(|text| !text.trim().is_empty()) {
            self.tool_expansions.entry(source).or_insert(expansion);
        }
        if !is_error
            && let Some(display) = edit_display.and_then(|value| format_edit(value, self.language))
        {
            self.tool_expansions.insert(source, display);
            self.tool_visuals.edits.insert(source);
        }
        self.transcript_render_cache = None;
        self.refresh_transcript_height();
    }

    #[cfg(test)]
    pub(super) fn note_tool_requested(
        &mut self,
        name: &str,
        detail: Option<&str>,
        expansion: Option<String>,
    ) {
        self.note_tool_call_requested(None, name, detail, expansion);
    }

    #[cfg(test)]
    pub(super) fn note_tool_completed(
        &mut self,
        name: &str,
        detail: Option<&str>,
        expansion: Option<String>,
        is_error: bool,
    ) {
        self.note_tool_call_completed(None, name, detail, expansion, is_error, None);
    }
}

/// Only consume receipts emitted after successful writes, never tool arguments.
pub(crate) fn receipt(name: &str, output: &str, is_error: bool) -> Option<serde_json::Value> {
    if is_error || !matches!(name, "create_file" | "edit_file" | "file_changes" | "lsp") {
        return None;
    }
    let display = willdeep_core::tools::edit_display::extract_edit_display(output, is_error)?;
    serde_json::to_value(display).ok()
}

/// Middle lines of a large per-file diff collapse to one marker row (#203).
fn elide_diff_body_lines(mut lines: Vec<String>, language: Language) -> Vec<String> {
    const MAX_VISIBLE: usize = 96;
    const HEAD: usize = 36;
    const TAIL: usize = 36;
    if lines.len() <= MAX_VISIBLE {
        return lines;
    }
    let omitted = lines.len().saturating_sub(HEAD + TAIL);
    let marker = language
        .text(
            "… 省略 {n} 行（点击行可展开完整差异）",
            "… {n} lines omitted (expand the row for the full diff)",
            "… {n} 行を省略（行を展開すると差分全体）",
        )
        .replace("{n}", &omitted.to_string());
    let tail = lines.split_off(lines.len() - TAIL);
    lines.truncate(HEAD);
    lines.push(marker);
    lines.extend(tail);
    lines
}

/// Convert a committed receipt into bounded, literal text. No Markdown or terminal escapes.
fn format_edit(value: &serde_json::Value, language: Language) -> Option<String> {
    const MAX_FILES: usize = 32;
    const MAX_BYTES: usize = 128 * 1024;
    let files = value.get("files")?.as_array()?;
    if files.is_empty() || files.len() > MAX_FILES {
        return None;
    }
    let mut lines = Vec::new();
    let mut bytes = 0usize;
    for file in files {
        let path = file.get("path")?.as_str()?;
        let diff = file.get("diff")?.as_str()?;
        bytes = bytes.checked_add(path.len())?.checked_add(diff.len())?;
        if bytes > MAX_BYTES {
            return None;
        }
        let added = file.get("added")?.as_u64()?;
        let removed = file.get("removed")?.as_u64()?;
        lines.push(format!("{}  +{added} −{removed}", safe_text(path)));
        let (mut old, mut new) = (0u64, 0u64);
        let path_line = lines.len();
        let mut file_body = Vec::new();
        for line in diff.lines() {
            if let Some(hunk) = line.strip_prefix("@@ -") {
                let (left, right) = hunk.split_once(" +")?;
                old = left.split(',').next()?.parse().ok()?;
                new = right.split([',', ' ']).next()?.parse().ok()?;
                file_body.push(safe_text(line));
                continue;
            }
            let (left, right) = match line.as_bytes().first() {
                Some(b'-') => {
                    let n = old;
                    old = old.saturating_add(1);
                    (n.to_string(), String::new())
                }
                Some(b'+') => {
                    let n = new;
                    new = new.saturating_add(1);
                    (String::new(), n.to_string())
                }
                Some(b' ') => {
                    let pair = (old.to_string(), new.to_string());
                    old = old.saturating_add(1);
                    new = new.saturating_add(1);
                    pair
                }
                _ => {
                    file_body.push(safe_text(line));
                    continue;
                }
            };
            file_body.push(format!("{left:>5} │ {right:>5} │ {}", safe_text(line)));
        }
        lines.splice(
            path_line..path_line,
            elide_diff_body_lines(file_body, language),
        );
        if file.get("truncated")?.as_bool()? {
            lines.push(
                language
                    .text(
                        "… 差异已截断",
                        "… Diff truncated",
                        "… 差分は省略されています",
                    )
                    .to_owned(),
            );
        }
    }
    Some(lines.join("\n"))
}

fn safe_text(value: &str) -> String {
    diff_review_ui::terminal_safe_diff_text(
        &willdeep_core::judge::redact_credentials_preserving_layout(value),
    )
}

pub(super) fn detail_lines(detail: &str, diff: bool) -> Text<'static> {
    Text::from(
        detail
            .split('\n')
            .map(|line| {
                let color = if !diff {
                    Color::Gray
                } else if line.starts_with("@@") {
                    Color::Yellow
                } else if let Some(body) = line.splitn(3, " │ ").nth(2) {
                    match body.as_bytes().first() {
                        Some(b'+') => Color::Green,
                        Some(b'-') => Color::Red,
                        _ => Color::Gray,
                    }
                } else {
                    Color::Cyan
                };
                Line::styled(line.to_owned(), Style::default().fg(color))
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
#[path = "tool_visuals_tests.rs"]
mod tests;
