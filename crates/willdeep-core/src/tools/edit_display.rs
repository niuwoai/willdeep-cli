//! Bounded display receipts made from the snapshots of successful core writes.
//! These are presentation data, not write-set, version, or verification evidence.

use serde::{Deserialize, Serialize};

pub const MAX_FILES: usize = 32;
pub const MAX_FILE_DIFF_BYTES: usize = 8 * 1024;
pub const MAX_TOTAL_DIFF_BYTES: usize = 64 * 1024;
const CONTEXT_LINES: usize = 3;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DIFF_WORK: usize = 2_000_000;
const MAX_EDIT_DISTANCE: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditDisplay {
    pub files: Vec<FileEditDisplay>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEditDisplay {
    /// The resolved workspace-relative path used by the write receipt.
    pub path: String,
    /// Unified hunks without file headers; unchanged context starts with a space.
    pub diff: String,
    /// Counts cover the entire edit script, even when display bytes are omitted.
    pub added: usize,
    pub removed: usize,
    /// Display bytes/files were omitted, or the bounded diff search fell back to
    /// a replacement script (whose counts need not be the minimal line counts).
    pub truncated: bool,
}

/// Extract only an explicit successful write receipt. Never consult arguments,
/// files, shell commands, or a model. Callers must pass the event's error flag.
/// Legacy plain text, previews, errors, and malformed receipts return `None`.
/// The same display limits are enforced again for persisted/untrusted outputs.
pub fn extract_edit_display(output: &str, is_error: bool) -> Option<EditDisplay> {
    if is_error {
        return None;
    }
    #[derive(Deserialize)]
    struct Receipt {
        status: String,
        edit_display: EditDisplay,
    }
    let receipt: Receipt = serde_json::from_str(output).ok()?;
    if !matches!(receipt.status.as_str(), "created" | "applied") {
        return None;
    }
    let mut display = receipt.edit_display;
    if display.files.is_empty()
        || display.files.iter().any(|file| {
            file.path.is_empty()
                || file.path.len() > MAX_PATH_BYTES
                || super::validate_workspace_relative(&file.path).is_err()
        })
    {
        return None;
    }
    bound_files(&mut display);
    Some(display)
}

/// The caller supplies the bytes actually installed, only after its complete
/// write/validation/cleanup succeeds. This function performs no I/O.
pub(super) fn from_snapshots<'a>(
    changes: impl IntoIterator<Item = (&'a str, &'a [u8], &'a [u8])>,
) -> EditDisplay {
    let mut files: Vec<FileEditDisplay> = Vec::new();
    let mut remaining = MAX_TOTAL_DIFF_BYTES;
    for (path, before, after) in changes {
        if files.len() == MAX_FILES {
            if let Some(last) = files.last_mut() {
                last.truncated = true;
            }
            break;
        }
        let file = file_diff(path, before, after, remaining.min(MAX_FILE_DIFF_BYTES));
        remaining = remaining.saturating_sub(file.diff.len());
        files.push(file);
    }
    EditDisplay { files }
}

fn bound_files(display: &mut EditDisplay) {
    if display.files.len() > MAX_FILES {
        display.files.truncate(MAX_FILES);
        display.files[MAX_FILES - 1].truncated = true;
    }
    let mut remaining = MAX_TOTAL_DIFF_BYTES;
    for file in &mut display.files {
        let cap = remaining.min(MAX_FILE_DIFF_BYTES);
        if file.diff.len() > cap {
            let mut end = cap;
            while !file.diff.is_char_boundary(end) {
                end -= 1;
            }
            file.diff.truncate(end);
            file.truncated = true;
        }
        remaining -= file.diff.len();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Context,
    Remove,
    Add,
}

#[derive(Clone, Copy, Debug)]
struct Line<'a> {
    kind: Kind,
    text: &'a str,
}

fn lines<'a>(kind: Kind, text: &[&'a str]) -> impl Iterator<Item = Line<'a>> {
    text.iter().map(move |text| Line { kind, text })
}

fn file_diff(path: &str, before: &[u8], after: &[u8], cap: usize) -> FileEditDisplay {
    let mut file = FileEditDisplay {
        path: path.to_owned(),
        diff: String::new(),
        added: 0,
        removed: 0,
        truncated: false,
    };
    // Current writers validate UTF-8 before committing. Do not fabricate a lossy
    // text diff if a future writer hands us binary bytes after a successful write.
    let (Ok(before), Ok(after)) = (std::str::from_utf8(before), std::str::from_utf8(after)) else {
        file.truncated = true;
        return file;
    };
    let old: Vec<_> = before.split_inclusive('\n').collect();
    let new: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    let middle = shortest_edit(&old[prefix..old_end], &new[prefix..new_end]);
    let middle = middle.unwrap_or_else(|| {
        file.truncated = true;
        lines(Kind::Remove, &old[prefix..old_end])
            .chain(lines(Kind::Add, &new[prefix..new_end]))
            .collect()
    });
    let script: Vec<_> = lines(Kind::Context, &old[..prefix])
        .chain(middle)
        .chain(lines(Kind::Context, &old[old_end..]))
        .collect();
    file.added = script.iter().filter(|line| line.kind == Kind::Add).count();
    file.removed = script
        .iter()
        .filter(|line| line.kind == Kind::Remove)
        .count();
    let (diff, truncated) = render_hunks(&script, cap);
    file.diff = diff;
    file.truncated |= truncated;
    file
}

/// Myers line diff with bounded trace memory and search work. Long unchanged
/// prefixes/suffixes have already been removed. Exhaustion uses a valid, marked
/// replacement script rather than claiming a minimal diff was computed.
fn shortest_edit<'a>(old: &[&'a str], new: &[&'a str]) -> Option<Vec<Line<'a>>> {
    if old.is_empty() {
        return Some(lines(Kind::Add, new).collect());
    }
    if new.is_empty() {
        return Some(lines(Kind::Remove, old).collect());
    }
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let mut work = 0;
    for depth in 0..=(old.len() + new.len()).min(MAX_EDIT_DISTANCE) {
        let mut row = Vec::with_capacity(depth + 1);
        for index in 0..=depth {
            work += 1;
            if work > MAX_DIFF_WORK {
                return None;
            }
            let mut x = if depth == 0 {
                0
            } else {
                let previous = &trace[depth - 1];
                if index == 0 || (index < depth && previous[index - 1] < previous[index]) {
                    previous[index]
                } else {
                    previous[index - 1] + 1
                }
            };
            let diagonal = 2 * index as isize - depth as isize;
            let mut y = (x as isize - diagonal) as usize;
            while x < old.len() && y < new.len() && old[x] == new[y] {
                work += 1;
                if work > MAX_DIFF_WORK {
                    return None;
                }
                x += 1;
                y += 1;
            }
            row.push(x);
            if x >= old.len() && y >= new.len() {
                trace.push(row);
                return Some(backtrack(old, new, &trace));
            }
        }
        trace.push(row);
    }
    None
}

fn backtrack<'a>(old: &[&'a str], new: &[&'a str], trace: &[Vec<usize>]) -> Vec<Line<'a>> {
    let (mut x, mut y) = (old.len(), new.len());
    let mut script = Vec::new();
    for depth in (1..trace.len()).rev() {
        let diagonal = x as isize - y as isize;
        let index = ((diagonal + depth as isize) / 2) as usize;
        let previous = &trace[depth - 1];
        let insertion = index == 0 || (index < depth && previous[index - 1] < previous[index]);
        let previous_x = if insertion {
            previous[index]
        } else {
            previous[index - 1]
        };
        let previous_diagonal = diagonal + if insertion { 1 } else { -1 };
        let previous_y = (previous_x as isize - previous_diagonal) as usize;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            script.push(Line {
                kind: Kind::Context,
                text: old[x],
            });
        }
        if insertion {
            y -= 1;
            script.push(Line {
                kind: Kind::Add,
                text: new[y],
            });
        } else {
            x -= 1;
            script.push(Line {
                kind: Kind::Remove,
                text: old[x],
            });
        }
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        script.push(Line {
            kind: Kind::Context,
            text: old[x],
        });
    }
    script.reverse();
    script
}

struct BoundedText {
    text: String,
    cap: usize,
    truncated: bool,
}

impl BoundedText {
    fn append(&mut self, text: &str) {
        if self.truncated {
            return;
        }
        let mut end = text.len().min(self.cap.saturating_sub(self.text.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&text[..end]);
        self.truncated = end != text.len();
    }
}

fn render_hunks(script: &[Line<'_>], cap: usize) -> (String, bool) {
    let mut hunks: Vec<std::ops::Range<usize>> = Vec::new();
    for (index, line) in script.iter().enumerate() {
        if line.kind == Kind::Context {
            continue;
        }
        let start = index.saturating_sub(CONTEXT_LINES);
        let end = (index + 1 + CONTEXT_LINES).min(script.len());
        if let Some(last) = hunks.last_mut()
            && start <= last.end
        {
            last.end = end;
        } else {
            hunks.push(start..end);
        }
    }
    let mut output = BoundedText {
        text: String::new(),
        cap,
        truncated: false,
    };
    let (mut cursor, mut old_line, mut new_line) = (0, 1, 1);
    for hunk in hunks {
        for line in &script[cursor..hunk.start] {
            old_line += usize::from(line.kind != Kind::Add);
            new_line += usize::from(line.kind != Kind::Remove);
        }
        let body = &script[hunk.clone()];
        let old_count = body.iter().filter(|line| line.kind != Kind::Add).count();
        let new_count = body.iter().filter(|line| line.kind != Kind::Remove).count();
        output.append(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_line - usize::from(old_count == 0),
            old_count,
            new_line - usize::from(new_count == 0),
            new_count,
        ));
        for line in body {
            output.append(match line.kind {
                Kind::Context => " ",
                Kind::Remove => "-",
                Kind::Add => "+",
            });
            output.append(line.text);
            if !line.text.ends_with('\n') {
                output.append("\n\\ No newline at end of file\n");
            }
        }
        old_line += old_count;
        new_line += new_count;
        cursor = hunk.end;
        if output.truncated {
            break;
        }
    }
    (output.text, output.truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diff(before: &str, after: &str) -> FileEditDisplay {
        from_snapshots([("a.txt", before.as_bytes(), after.as_bytes())])
            .files
            .remove(0)
    }

    #[test]
    fn adjacent_changes_share_real_context_and_correct_counts() {
        let file = diff("a\nb\nc\nd\ne\n", "a\nB\nc\nD\ne\n");
        assert_eq!(file.diff, "@@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n-d\n+D\n e\n");
        assert_eq!((file.added, file.removed, file.truncated), (2, 2, false));
        assert!(!file.diff.contains("---"));
    }

    #[test]
    fn distant_changes_have_separate_numbered_hunks() {
        let before = (1..=24).map(|n| format!("line {n}\n")).collect::<String>();
        let after = before
            .replace("line 5\n", "changed 5\nextra\n")
            .replace("line 20\n", "changed 20\n");
        let file = diff(&before, &after);
        assert!(file.diff.starts_with("@@ -2,7 +2,8 @@\n"));
        assert!(file.diff.contains("@@ -17,7 +18,7 @@\n"));
        assert_eq!((file.added, file.removed), (3, 2));
        assert!(!file.diff.contains("line 12"));
    }

    #[test]
    fn empty_creation_deletion_and_missing_newline_are_exact() {
        assert_eq!(diff("", "").diff, "");
        assert_eq!(diff("", "").added, 0);
        assert!(!diff("", "").truncated);
        assert_eq!(diff("", "one\n").diff, "@@ -0,0 +1,1 @@\n+one\n");
        assert_eq!(diff("one\n", "").diff, "@@ -1,1 +0,0 @@\n-one\n");
        assert_eq!(
            diff("one", "one\n").diff,
            "@@ -1,1 +1,1 @@\n-one\n\\ No newline at end of file\n+one\n"
        );
    }

    #[test]
    fn diff_limits_preserve_utf8_and_full_counts_and_mark_omitted_files() {
        let content = format!("{}\n", "中文".repeat(MAX_FILE_DIFF_BYTES));
        let display =
            from_snapshots((0..MAX_FILES + 1).map(|_| ("a.txt", &b""[..], content.as_bytes())));
        assert_eq!(display.files.len(), MAX_FILES);
        assert!(display.files.last().unwrap().truncated);
        assert!(
            display
                .files
                .iter()
                .all(|file| file.diff.len() <= MAX_FILE_DIFF_BYTES
                    && file.truncated
                    && file.added == 1
                    && file.removed == 0)
        );
        assert!(
            display
                .files
                .iter()
                .map(|file| file.diff.len())
                .sum::<usize>()
                <= MAX_TOTAL_DIFF_BYTES
        );
        assert_eq!(file_diff("a", b"a", b"b", 0).diff, "");
        assert!(file_diff("a", b"a", b"b", 0).truncated);
        for cap in 0..48 {
            let file = file_diff("a", b"", "中文\n".as_bytes(), cap);
            assert!(file.diff.len() <= cap);
            assert!("@@ -0,0 +1,1 @@\n+中文\n".starts_with(&file.diff));
        }
    }

    #[test]
    fn extraction_requires_explicit_success_and_rebounds_persisted_data() {
        let display = from_snapshots([("a.txt", &b"old\n"[..], &b"new\n"[..])]);
        for status in ["applied", "created"] {
            let output = json!({"status":status,"edit_display":display}).to_string();
            assert_eq!(extract_edit_display(&output, false), Some(display.clone()));
            assert_eq!(extract_edit_display(&output, true), None);
        }
        for status in [
            "preview",
            "rejected",
            "rolled_back",
            "partial_failure",
            "applied_with_cleanup_failure",
        ] {
            let output = json!({"status":status,"edit_display":display}).to_string();
            assert_eq!(extract_edit_display(&output, false), None);
        }
        for output in ["created a.txt (4 bytes)", "{}", "not json"] {
            assert_eq!(extract_edit_display(output, false), None);
        }
        let mut oversized = display.clone();
        oversized.files[0].diff = "中".repeat(MAX_TOTAL_DIFF_BYTES);
        oversized.files = vec![oversized.files[0].clone(); MAX_FILES + 1];
        let output = json!({"status":"applied","edit_display":oversized}).to_string();
        let bounded = extract_edit_display(&output, false).unwrap();
        assert_eq!(bounded.files.len(), MAX_FILES);
        assert!(
            bounded
                .files
                .iter()
                .all(|file| file.truncated && file.diff.len() <= MAX_FILE_DIFF_BYTES)
        );
        assert!(
            bounded
                .files
                .iter()
                .map(|file| file.diff.len())
                .sum::<usize>()
                <= MAX_TOTAL_DIFF_BYTES
        );
        for path in ["", "../outside", "/absolute"] {
            let output = json!({"status":"applied","edit_display":{"files":[{"path":path,"diff":"","added":0,"removed":0,"truncated":false}]}}).to_string();
            assert_eq!(extract_edit_display(&output, false), None);
        }
    }

    #[test]
    fn bounded_search_fallback_is_marked_and_keeps_outer_context() {
        let before = format!("context\n{}tail\n", "old\n".repeat(MAX_EDIT_DISTANCE));
        let after = format!("context\n{}tail\n", "new\n".repeat(MAX_EDIT_DISTANCE));
        let file = diff(&before, &after);
        assert!(file.truncated);
        assert!(file.diff.contains(" context\n"));
        assert!(file.diff.ends_with(" tail\n"));
        assert_eq!(
            (file.added, file.removed),
            (MAX_EDIT_DISTANCE, MAX_EDIT_DISTANCE)
        );
    }

    #[test]
    fn small_scripts_reconstruct_both_snapshots_and_have_minimal_counts() {
        let mut samples = vec![Vec::new()];
        for length in 1..=3 {
            for mut encoded in 0..3usize.pow(length) {
                let mut sample = Vec::new();
                for _ in 0..length {
                    sample.push(["a\n", "b\n", "c\n"][encoded % 3]);
                    encoded /= 3;
                }
                samples.push(sample);
            }
        }
        for old in &samples {
            for new in &samples {
                let script = shortest_edit(old, new).unwrap();
                assert_eq!(
                    script
                        .iter()
                        .filter(|line| line.kind != Kind::Add)
                        .map(|line| line.text)
                        .collect::<Vec<_>>(),
                    *old
                );
                assert_eq!(
                    script
                        .iter()
                        .filter(|line| line.kind != Kind::Remove)
                        .map(|line| line.text)
                        .collect::<Vec<_>>(),
                    *new
                );
                let mut lcs = vec![vec![0; new.len() + 1]; old.len() + 1];
                for (i, a) in old.iter().enumerate() {
                    for (j, b) in new.iter().enumerate() {
                        lcs[i + 1][j + 1] = if a == b {
                            lcs[i][j] + 1
                        } else {
                            lcs[i][j + 1].max(lcs[i + 1][j])
                        };
                    }
                }
                assert_eq!(
                    script
                        .iter()
                        .filter(|line| line.kind != Kind::Context)
                        .count(),
                    old.len() + new.len() - 2 * lcs[old.len()][new.len()]
                );
            }
        }
    }
}
