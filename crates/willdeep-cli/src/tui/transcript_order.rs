//! Reorder existing records without deleting their receipt or source identity.
use super::*;

impl App {
    /// Move one existing record, with all row-keyed state, to the transcript tail.
    /// An out-of-range index or a record already at the tail is left unchanged.
    /// This does not append a new message or update its submission time.
    pub(super) fn move_transcript_row_to_end(&mut self, index: usize) -> usize {
        let count = self.transcript.len();
        if index >= count || index + 1 == count {
            return index;
        }
        let last = count - 1;
        let remap = |row: usize| {
            if row == index {
                last
            } else if row > index && row < count {
                row - 1
            } else {
                row
            }
        };
        fn remap_map<V>(map: &mut BTreeMap<usize, V>, remap: impl Fn(usize) -> usize) {
            *map = std::mem::take(map)
                .into_iter()
                .map(|(row, value)| (remap(row), value))
                .collect();
        }

        // Capture sources against the still-painted frame before changing order.
        self.viewport_move_record_to_end(index);
        self.transcript[index..].rotate_left(1);
        self.transcript_attachments.resize_with(count, Vec::new);
        self.transcript_attachments[index..].rotate_left(1);
        self.transcript_models.resize_with(count, || None);
        self.transcript_models[index..].rotate_left(1);
        self.message_times.move_row_to_end(index, count);

        for echo in &mut self.pending_runtime_echoes {
            echo.record = echo.record.map(remap);
        }
        for queued in &mut self.queued_prompts {
            queued.transcript_row = queued.transcript_row.map(remap);
        }
        remap_map(&mut self.expanded_user_messages, remap);
        remap_map(&mut self.tool_expansions, remap);
        self.tool_visuals.remap(|source| Some(remap(source)));
        self.expanded_tool_rows = std::mem::take(&mut self.expanded_tool_rows)
            .into_iter()
            .map(remap)
            .collect();

        // The floor is a prefix boundary, not the identity of its first live row.
        // A record moved out of that prefix becomes visible at the tail.
        if index < self.archive_floor {
            self.archive_floor -= 1;
        }
        if let Some(search) = &mut self.search {
            let selected = search.matches.get(search.selected).copied().map(remap);
            search.matches.iter_mut().for_each(|row| *row = remap(*row));
            search.matches.sort_unstable();
            search.selected = selected
                .and_then(|row| search.matches.iter().position(|index| *index == row))
                .unwrap_or(0);
        }

        // Geometry and hit targets refer to the previous frame. Rebuild them on
        // the next paint; retain that frame's sources for selection/reading.
        self.transcript_render_cache = None;
        self.user_message_controls.clear();
        self.user_message_hit_rows.clear();
        self.tool_row_controls.clear();
        self.tool_row_hits.clear();
        self.attachment_controls.clear();
        self.attachment_hit_rows.clear();
        self.thought_hit_rows.clear();
        self.pending_chat_click = None;
        self.refresh_transcript_height();
        last
    }
}

#[cfg(test)]
#[path = "transcript_order_tests.rs"]
mod tests;
