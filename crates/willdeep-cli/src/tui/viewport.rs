//! Reading and selection state shared by full and compact transcript layouts.
use super::rendering::source::{self, SourceFrame, SourceId, SourcePart, SourcePoint};
use super::*;
use ratatui::Frame;

#[derive(Clone, Copy)]
struct ReadingAnchor {
    point: SourcePoint,
    screen_row: usize,
}

#[derive(Clone, Copy)]
struct SourceSelection {
    anchor: SourcePoint,
    head: SourcePoint,
}

// Group each attachment's label and body together while retaining SourcePart's
// prefix/body/attachment/detail ordering.
fn selection_position(part: SourcePart, byte: usize) -> (SourcePart, SourcePart, usize) {
    let group = match part {
        SourcePart::AttachmentBody(index) => SourcePart::AttachmentLabel(index),
        _ => part,
    };
    (group, part, byte)
}

struct SourceSelectionRange {
    id: SourceId,
    part: SourcePart,
    start: usize,
    end: usize,
}

impl SourceSelectionRange {
    fn contains(&self, point: SourcePoint) -> bool {
        point.id == self.id
            && point.part == self.part
            && (self.start..=self.end).contains(&point.byte)
    }
}

impl SourceSelection {
    fn ranges(self, order: &[SourceId], frame: &SourceFrame) -> Vec<SourceSelectionRange> {
        let (Some(anchor_rank), Some(head_rank)) = (
            order.iter().position(|id| *id == self.anchor.id),
            order.iter().position(|id| *id == self.head.id),
        ) else {
            return Vec::new();
        };
        let key = |rank, point: SourcePoint| (rank, selection_position(point.part, point.byte));
        let (start_rank, start, end_rank, end) =
            if key(anchor_rank, self.anchor) <= key(head_rank, self.head) {
                (anchor_rank, self.anchor, head_rank, self.head)
            } else {
                (head_rank, self.head, anchor_rank, self.anchor)
            };
        let ranks = order
            .iter()
            .enumerate()
            .map(|(rank, id)| (*id, rank))
            .collect::<BTreeMap<_, _>>();
        // Bound each existing part separately: reconciliation can add attachments
        // to a selected survivor, and those new bytes were never selected.
        let mut lengths = frame
            .parts
            .iter()
            .map(|(key, text)| (*key, text.len()))
            .collect::<BTreeMap<_, _>>();
        // Selectable role prefixes have mapped bytes but no raw part entry.
        // Keep raw lengths for folded bodies and supplement those mapped parts.
        for cell in frame.rows.iter().flat_map(|row| &row.cells) {
            lengths
                .entry((cell.source.point.id, cell.source.point.part))
                .and_modify(|length| *length = (*length).max(cell.source.end))
                .or_insert(cell.source.end);
        }
        lengths
            .iter()
            .filter_map(|(&(id, part), &length)| {
                let rank = *ranks.get(&id)?;
                let last_byte = length.checked_sub(1)?;
                let point = |byte| SourcePoint { id, part, byte };
                if key(rank, point(0)) > key(end_rank, end)
                    || key(rank, point(last_byte)) < key(start_rank, start)
                {
                    return None;
                }
                Some(SourceSelectionRange {
                    id,
                    part,
                    start: if id == start.id && part == start.part {
                        start.byte
                    } else {
                        0
                    },
                    end: if id == end.id && part == end.part {
                        end.byte.min(last_byte)
                    } else {
                        last_byte
                    },
                })
            })
            .collect()
    }
}

#[derive(Default)]
pub(super) struct ViewportState {
    pub(super) frame: SourceFrame,
    records: Vec<SourceId>,
    next_record: usize,
    next_stream: u64,
    stream: Option<(StreamKind, SourceId)>,
    held_stream: Option<SourceId>,
    reading: Option<ReadingAnchor>,
    selection: Option<SourceSelection>,
    // A reordered selection keeps its original record membership; fresh drags
    // use the current record order, never the numerical value of a stable ID.
    selection_order: Option<Vec<SourceId>>,
    // Identity merges can leave disjoint ranges on one surviving source.
    selection_ranges: Option<Vec<SourceSelectionRange>>,
    selection_text: Option<String>,
    resolved_selection: Option<ChatSelection>,
    last_offset: usize,
    last_height: usize,
    observed: BTreeMap<(SourceId, SourcePart), String>,
    observed_once: bool,
    new_content: usize,
    indicator: Rect,
}

impl ViewportState {
    fn ensure_records(&mut self, count: usize) {
        self.records.truncate(count);
        while self.records.len() < count {
            let id = SourceId::Record(self.next_record);
            self.next_record += 1;
            self.records.push(id);
        }
    }

    fn capture_reading(&mut self, offset: usize) {
        self.last_offset = offset;
        self.reading = self
            .frame
            .rows
            .iter()
            .enumerate()
            .skip(offset)
            .take(self.last_height.max(1))
            .find(|(_, row)| !row.cells.is_empty())
            .or_else(|| self.frame.rows.get(offset).map(|row| (offset, row)))
            .map(|(row, mapped)| ReadingAnchor {
                point: mapped
                    .cells
                    .first()
                    .map_or(mapped.fallback, |cell| cell.source.point),
                screen_row: row - offset,
            });
    }

    fn capture_selection(&mut self, selection: Option<ChatSelection>) {
        if selection.is_some() && selection == self.resolved_selection && self.selection.is_some() {
            return;
        }
        self.selection_order = None;
        self.selection_ranges = None;
        self.selection = selection.and_then(|selection| {
            Some(SourceSelection {
                anchor: self.frame.point_at(selection.anchor)?,
                head: self.frame.point_at(selection.head)?,
            })
        });
        self.selection_text = self
            .selection
            .map(|selection| self.frame.selected_text(selection.anchor, selection.head));
        self.resolved_selection = selection;
    }

    fn source_order(&self) -> Vec<SourceId> {
        let mut order = vec![SourceId::Archive];
        order.extend(self.records.iter().copied());
        let mut seen = order.iter().copied().collect::<BTreeSet<_>>();
        for row in &self.frame.rows {
            if seen.insert(row.fallback.id) {
                order.push(row.fallback.id);
            }
        }
        order
    }

    fn migrate(&mut self, from: SourceId, to: SourceId) {
        if let Some(ranges) = &mut self.selection_ranges {
            // Keep both copies' bounds, including gaps between selected bytes.
            // Picking one position for a duplicate ID would lose that union.
            for range in ranges {
                if range.id == from {
                    range.id = to;
                }
            }
        }
        if let Some(anchor) = self.reading.as_mut()
            && anchor.point.id == from
        {
            anchor.point.id = to;
        }
        if let Some(selection) = self.selection.as_mut() {
            if selection.anchor.id == from {
                selection.anchor.id = to;
            }
            if selection.head.id == from {
                selection.head.id = to;
            }
        }
        if let Some(order) = &mut self.selection_order {
            let mut seen = BTreeSet::new();
            for id in order.iter_mut() {
                if *id == from {
                    *id = to;
                }
            }
            order.retain(|id| seen.insert(*id));
        }
        self.frame.rebase(&BTreeMap::from([(from, to)]));
        self.observed = std::mem::take(&mut self.observed)
            .into_iter()
            .map(|((id, part), text)| ((if id == from { to } else { id }, part), text))
            .collect();
    }
}

impl App {
    /// Call whenever a different transcript/session replaces the current one.
    pub(super) fn reset_transcript_viewport(&mut self) {
        self.viewport = ViewportState::default();
        self.thought_hit_rows.clear();
        self.follow_bottom = true;
        self.scroll_from_bottom = 0;
        self.transcript_render_offset = 0;
        self.chat_selection = None;
        self.transcript_render_cache = None;
    }

    /// Call AFTER an explicit scroll/search jump changes scroll_from_bottom.
    /// Height compensation and folding must not call this: those preserve sources.
    pub(super) fn viewport_scrolled(&mut self) {
        if self.follow_bottom {
            self.viewport_return_to_latest();
            return;
        }
        let offset = self.max_scroll().saturating_sub(self.scroll_from_bottom);
        self.viewport.capture_reading(offset);
    }

    /// Freeze the frame actually under the pointer, even if an unpainted delta
    /// has already changed the calculated transcript height.
    pub(super) fn viewport_pause_for_selection(&mut self) {
        // Mouse Down starts a new gesture even when merged endpoints coincide.
        self.viewport_clear_selection();
        self.follow_bottom = false;
        self.viewport.capture_reading(self.transcript_render_offset);
    }

    /// Shared End/Ctrl+End/click action. Does not change transcript geometry.
    pub(super) fn viewport_return_to_latest(&mut self) {
        self.follow_bottom = true;
        self.scroll_from_bottom = 0;
        self.viewport.reading = None;
        self.viewport.new_content = 0;
        self.viewport.indicator = Rect::default();
    }

    /// Call BEFORE reordering transcript[index]. Identity and source byte offsets
    /// survive; deletion's tombstones and adjacent-record fallback do not apply.
    pub(super) fn viewport_move_record_to_end(&mut self, index: usize) {
        self.viewport.ensure_records(self.transcript.len());
        if index >= self.viewport.records.len() {
            return;
        }
        self.viewport.capture_selection(self.chat_selection);
        if !self.follow_bottom && self.viewport.reading.is_none() {
            self.viewport.capture_reading(self.transcript_render_offset);
        }
        if self.viewport.selection.is_some()
            && self.viewport.selection_ranges.is_none()
            && self.viewport.selection_order.is_none()
        {
            self.viewport.selection_order = Some(self.viewport.source_order());
        }
        self.viewport.records[index..].rotate_left(1);
        self.thought_hit_rows.clear();
    }

    /// Reconcile two proven copies of the same message BEFORE deleting `from`.
    /// Keep its selected bytes and reading anchor on the surviving record `to`.
    pub(super) fn viewport_merge_record(&mut self, from: usize, to: usize) {
        self.viewport.ensure_records(self.transcript.len());
        let (Some(&from), Some(&to)) = (
            self.viewport.records.get(from),
            self.viewport.records.get(to),
        ) else {
            return;
        };
        self.viewport.capture_selection(self.chat_selection);
        if !self.follow_bottom && self.viewport.reading.is_none() {
            self.viewport.capture_reading(self.transcript_render_offset);
        }
        if self.viewport.selection_ranges.is_none()
            && let Some(selection) = self.viewport.selection
        {
            let order = self
                .viewport
                .selection_order
                .take()
                .unwrap_or_else(|| self.viewport.source_order());
            self.viewport.selection_ranges = Some(selection.ranges(&order, &self.viewport.frame));
        }
        self.viewport.migrate(from, to);
        self.thought_hit_rows.clear();
        self.transcript_render_cache = None;
    }

    /// Call BEFORE removing transcript[index], including removal of a queued echo.
    pub(super) fn viewport_remove_record(&mut self, index: usize) {
        self.viewport.ensure_records(self.transcript.len());
        if index >= self.viewport.records.len() {
            return;
        }
        self.viewport.capture_selection(self.chat_selection);
        if !self.follow_bottom && self.viewport.reading.is_none() {
            self.viewport.capture_reading(self.transcript_render_offset);
        }
        let removed = self.viewport.records.remove(index);
        self.thought_hit_rows.clear();
        if self
            .viewport
            .selection
            .is_some_and(|selection| selection.anchor.id == removed || selection.head.id == removed)
        {
            self.viewport.selection_text = None;
        }
        let neighbour = self
            .viewport
            .records
            .get(index)
            .or_else(|| {
                index
                    .checked_sub(1)
                    .and_then(|i| self.viewport.records.get(i))
            })
            .copied();
        if let Some(to) = neighbour {
            // Deleted source falls back to the adjacent surviving record's title.
            if let Some(anchor) = self.viewport.reading.as_mut()
                && anchor.point.id == removed
            {
                anchor.point = SourcePoint {
                    id: to,
                    part: SourcePart::Body,
                    byte: 0,
                };
            }
            if let Some(selection) = self.viewport.selection.as_mut() {
                for point in [&mut selection.anchor, &mut selection.head] {
                    if point.id == removed {
                        *point = SourcePoint {
                            id: to,
                            part: SourcePart::Body,
                            byte: 0,
                        };
                    }
                }
            }
        } else {
            self.viewport.reading = None;
            self.viewport.selection = None;
        }
        self.transcript_render_cache = None;
    }

    /// Call AFTER appending the history record, BEFORE clearing/changing the
    /// live stream. The caller knows which record represents that stream; no
    /// displayed-string matching is used. Body offsets exclude role/status labels.
    pub(super) fn viewport_commit_stream(&mut self, index: usize, kind: StreamKind) {
        self.viewport.ensure_records(self.transcript.len());
        let Some(&to) = self.viewport.records.get(index) else {
            return;
        };
        self.viewport.capture_selection(self.chat_selection);
        if !self.follow_bottom && self.viewport.reading.is_none() {
            self.viewport.capture_reading(self.transcript_render_offset);
        }
        let from = if kind == StreamKind::Reply && self.viewport.held_stream.is_some() {
            self.viewport.held_stream.take()
        } else if self
            .viewport
            .stream
            .is_some_and(|(stream_kind, _)| stream_kind == kind)
        {
            self.viewport.stream.take().map(|(_, id)| id)
        } else {
            None
        };
        if let Some(from) = from {
            self.viewport.migrate(from, to);
            self.transcript_render_cache = None;
        }
    }

    /// Keep a finalized concise paragraph distinct from the next live Reply.
    pub(super) fn viewport_hold_stream(&mut self) {
        self.viewport.capture_selection(self.chat_selection);
        let id = self
            .viewport
            .stream
            .take()
            .filter(|(kind, _)| *kind == StreamKind::Reply)
            .map(|(_, id)| id)
            .unwrap_or_else(|| {
                let id = SourceId::Stream(self.viewport.next_stream);
                self.viewport.next_stream += 1;
                id
            });
        self.viewport.held_stream = Some(id);
    }

    /// Message identity comes from the receipt's record, never its status label.
    pub(super) fn viewport_record_body(&self, index: usize) -> String {
        self.pending_runtime_echoes
            .iter()
            .find(|echo| echo.record == Some(index))
            .map_or_else(
                || {
                    self.queued_prompts
                        .iter()
                        .find(|queued| {
                            queued.remote.is_none()
                                && self.local_queue_record(queued) == Some(index)
                        })
                        .map_or_else(
                            || source::record_body(&self.transcript[index]).to_owned(),
                            |queued| terminal_safe_text(&queued.text),
                        )
                },
                |echo| terminal_safe_text(&echo.text),
            )
    }

    pub(super) fn viewport_begin_frame(&mut self) {
        if self.follow_bottom {
            self.viewport.reading = None;
            self.viewport.new_content = 0;
        } else if self.viewport.reading.is_none() {
            self.viewport.capture_reading(self.transcript_render_offset);
        }
        self.viewport.capture_selection(self.chat_selection);
        self.viewport.ensure_records(self.transcript.len());
    }

    /// Bind displayed thought headers to their real records after folding,
    /// wrapping and transient-row insertion. Body text is never a control.
    pub(super) fn update_thought_hits(&mut self, offset: usize, top: u16) {
        let records = self
            .viewport
            .records
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect::<BTreeMap<_, _>>();
        self.thought_hit_rows = self
            .viewport
            .frame
            .rows
            .iter()
            .enumerate()
            .skip(offset)
            .take(self.viewport_height)
            .filter_map(|(line, row)| {
                let point = row.cells.first()?.source.point;
                if point.part != SourcePart::Prefix {
                    return None;
                }
                let source = *records.get(&point.id)?;
                rendering::thought_entry_parts(self.transcript.get(source)?)?;
                Some((top.saturating_add((line - offset) as u16), source))
            })
            .collect();
    }

    pub(super) fn viewport_record_sources(&self, sources: &mut SourceFrame) {
        let ids = self
            .viewport
            .records
            .iter()
            .enumerate()
            .map(|(index, id)| (SourceId::Record(index), *id))
            .collect();
        sources.rebase(&ids);
    }

    pub(super) fn viewport_stream_id(&mut self) -> SourceId {
        if self.transient_thought.is_none()
            && self.held_narration.is_some()
            && let Some(id) = self.viewport.held_stream
        {
            return id;
        }
        if let Some((kind, id)) = self.viewport.stream
            && kind == self.transient_kind
        {
            return id;
        }
        let id = SourceId::Stream(self.viewport.next_stream);
        self.viewport.next_stream += 1;
        self.viewport.stream = Some((self.transient_kind, id));
        id
    }

    pub(super) fn viewport_finish_frame(&mut self, frame: SourceFrame) {
        // Observe original bodies, including currently folded/archived parts.
        // Layout, time labels, status animations and '+'/'−' never count as news.
        let mut observed = BTreeMap::new();
        for index in 0..self.transcript.len() {
            let id = self.viewport.records[index];
            observed.insert((id, SourcePart::Body), self.viewport_record_body(index));
            if let Some(detail) = self.tool_expansions.get(&index) {
                observed.insert((id, SourcePart::ToolDetail), detail.clone());
            }
            if let Some(attachments) = self.transcript_attachments.get(index) {
                for (index, attachment) in attachments.iter().enumerate() {
                    if let MessageAttachment::Text { content, .. } = attachment {
                        observed.insert((id, SourcePart::AttachmentBody(index)), content.clone());
                    }
                }
            }
        }
        let mut reply_delta = false;
        if self.transient_kind == StreamKind::Reply
            && self.transient_thought.is_some()
            && let Some((_, id)) = self.viewport.stream
            && let Some(body) = frame.parts.get(&(id, SourcePart::Body))
        {
            let key = (id, SourcePart::Body);
            reply_delta = !body.is_empty() && self.viewport.observed.get(&key) != Some(body);
            observed.insert(key, body.clone());
        }
        if let Some(id) = self.viewport.held_stream
            && let Some(held) = self.held_narration.as_ref()
        {
            observed.insert((id, SourcePart::Body), terminal_safe_text(&held.text));
        }
        let changed = observed
            .iter()
            .any(|(key, body)| !body.is_empty() && self.viewport.observed.get(key) != Some(body));
        if !self.follow_bottom && self.viewport.observed_once && changed {
            self.viewport.new_content = self.viewport.new_content.saturating_add(1);
        }
        self.viewport.observed = observed;
        self.viewport.observed_once = true;
        if self.transient_thought.is_none() {
            self.viewport.stream = None;
        }
        let max = self.max_scroll();
        let mut offset = if self.follow_bottom {
            max
        } else if let Some(anchor) = self.viewport.reading {
            frame
                .resolve(anchor.point)
                .map_or(self.viewport.last_offset, |point| {
                    point.row.saturating_sub(anchor.screen_row)
                })
                .min(max)
        } else {
            max.saturating_sub(self.scroll_from_bottom)
        };
        let remaining = max.saturating_sub(offset);
        // A resize or collapsed row can bring a reading anchor back to the bottom.
        // Keep the frame frozen while the user is selecting text, and allow an
        // explicit upward scroll to remain one wheel tick from the bottom.
        if !self.follow_bottom
            && !self.selection_mode
            && self.chat_selection.is_none()
            && (max <= Self::NEAR_BOTTOM_LINES
                || remaining == 0
                || (reply_delta && remaining == 1)
                || (remaining <= Self::NEAR_BOTTOM_LINES && remaining < self.scroll_from_bottom))
        {
            self.viewport_return_to_latest();
            offset = max;
        }
        self.scroll_from_bottom = max.saturating_sub(offset);
        self.transcript_render_offset = offset;
        if let Some(anchor) = self.viewport.reading.as_mut()
            && !frame.parts.keys().any(|(id, _)| *id == anchor.point.id)
            && !frame.aliases.contains_key(&anchor.point.id)
            && let Some(position) = frame.resolve(anchor.point)
            && let Some(point) = frame.point_at(position)
        {
            anchor.point = point;
        }
        self.viewport.frame = frame;
        self.viewport.last_offset = offset;
        self.viewport.last_height = self.viewport_height;
        if !self.follow_bottom && self.viewport.reading.is_none() {
            self.viewport.capture_reading(offset);
        }
        self.chat_selection = self.viewport.selection.and_then(|selection| {
            Some(ChatSelection {
                anchor: self.viewport.frame.resolve(selection.anchor)?,
                head: self.viewport.frame.resolve(selection.head)?,
            })
        });
        self.viewport.resolved_selection = self.chat_selection;
    }

    pub(super) fn viewport_clear_selection(&mut self) {
        self.viewport.selection = None;
        self.viewport.selection_order = None;
        self.viewport.selection_ranges = None;
        self.viewport.selection_text = None;
        self.viewport.resolved_selection = None;
    }

    pub(super) fn viewport_capture_selection(&mut self) {
        self.viewport.capture_selection(self.chat_selection);
    }

    pub(super) fn viewport_selected_text(&self) -> Option<String> {
        let selection = if self.chat_selection == self.viewport.resolved_selection {
            if let Some(text) = &self.viewport.selection_text {
                return Some(text.clone());
            }
            self.viewport.selection?
        } else {
            let selection = self.chat_selection?;
            SourceSelection {
                anchor: self.viewport.frame.point_at(selection.anchor)?,
                head: self.viewport.frame.point_at(selection.head)?,
            }
        };
        Some(
            self.viewport
                .frame
                .selected_text(selection.anchor, selection.head),
        )
    }

    /// Highlights only selectable source cells, matching provenance-based copy.
    /// The full/compact renderer calls this instead of coordinate-only highlighting.
    pub(super) fn highlight_chat_selection(&self, text: &mut Text<'static>, offset: usize) {
        let Some(selection) = self.chat_selection else {
            return;
        };
        let source = if self.chat_selection == self.viewport.resolved_selection {
            self.viewport.selection
        } else {
            self.viewport
                .frame
                .point_at(selection.anchor)
                .zip(self.viewport.frame.point_at(selection.head))
                .map(|(anchor, head)| SourceSelection { anchor, head })
        };
        let Some(source) = source else { return };
        let current_ranges;
        let ranges = if self.chat_selection == self.viewport.resolved_selection
            && let Some(ranges) = &self.viewport.selection_ranges
        {
            ranges
        } else {
            let current_order;
            let order = if self.chat_selection == self.viewport.resolved_selection
                && let Some(order) = &self.viewport.selection_order
            {
                order
            } else {
                current_order = self.viewport.source_order();
                &current_order
            };
            current_ranges = source.ranges(order, &self.viewport.frame);
            &current_ranges
        };
        for (local, line) in text.lines.iter_mut().enumerate() {
            let Some(mapped) = self.viewport.frame.rows.get(offset + local) else {
                continue;
            };
            let mut selected = Text::from(line.clone());
            for cell in &mapped.cells {
                if ranges.iter().any(|range| range.contains(cell.source.point)) {
                    rendering::highlight_text_selection_at_offset(
                        &mut selected,
                        (0, cell.columns.start),
                        (0, cell.columns.end),
                        0,
                    );
                }
            }
            if let Some(highlighted) = selected.lines.into_iter().next() {
                *line = highlighted;
            }
        }
    }

    /// Use the existing separator outside the body: the full chat's bottom border
    /// or the compact composer's top border. Reading never overwrites source rows
    /// or changes viewport geometry; the complete hint row remains clickable.
    pub(super) fn render_reading_indicator(&mut self, frame: &mut Frame<'_>) {
        self.viewport.indicator = Rect::default();
        if self.follow_bottom
            || self.scroll_from_bottom == 0
            || self.transcript_content.width == 0
            || self.transcript_content.height == 0
        {
            return;
        }
        let rect = Rect {
            y: if self.quiet_drawn {
                self.prompt_rect.y
            } else {
                self.transcript_rect.bottom().saturating_sub(1)
            },
            height: 1,
            ..self.transcript_content
        };
        if rect.y < self.transcript_content.bottom() || rect.y >= frame.area().bottom() {
            return;
        }
        let full_hint = if self.viewport.new_content > 0 {
            self.language.text(
                "有新内容 · 点击返回最新 (End)",
                "New content · Click for latest (End)",
                "新しい内容 · クリックで最新へ (End)",
            )
        } else {
            self.language.text(
                "正在回看 · 点击返回最新 (End)",
                "Reading history · Click for latest (End)",
                "履歴を表示中 · クリックで最新へ (End)",
            )
        };
        let short_hint = self.language.text(
            "点击返回最新 (End)",
            "Click for latest (End)",
            "クリックで最新へ (End)",
        );
        let text = if self.native_selection_mode {
            self.language.text(
                "Esc 恢复鼠标",
                "Esc restores mouse",
                "Esc でマウス操作を再開",
            )
        } else if UnicodeWidthStr::width(full_hint) > rect.width as usize {
            short_hint
        } else {
            full_hint
        };
        // Paragraph only writes its own characters; clear old border cells too,
        // including the continuation cells of wide glyphs.
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::Cyan).bg(Color::Black)),
            rect,
        );
        self.viewport.indicator = rect;
    }

    pub(super) fn reading_indicator_hit(&self, x: u16, y: u16) -> bool {
        let rect = self.viewport.indicator;
        rect.width > 0 && rect.height > 0 && x >= rect.x && x < rect.right() && y == rect.y
    }
}

#[cfg(test)]
#[path = "viewport_tests.rs"]
mod tests;
