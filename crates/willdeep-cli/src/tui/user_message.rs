use super::narration::FoldedTranscript;
use super::*;

pub(super) const PREVIEW_LINES: usize = 8;

impl App {
    pub(super) fn render_user_transcript(
        &self,
        folded: &FoldedTranscript,
        labels: &[Option<String>],
        search: Option<&str>,
        end: usize,
    ) -> (Text<'static>, Vec<(usize, usize)>) {
        let floor = self.archive_floor.min(self.transcript.len());
        let shift = usize::from(floor > 0);
        let users = self
            .transcript
            .iter()
            .enumerate()
            .skip(floor)
            .filter(|(_, row)| row.starts_with("You: "))
            .filter_map(|(source, row)| {
                let target = *folded.index_of.get(source - floor + shift)?;
                let expanded = self.expanded_user_messages.get(&source) == Some(row);
                Some((target, (source, expanded)))
            })
            .collect();
        message_time::render_with_user_folds(
            &folded.rows[..end],
            &labels[..end],
            search,
            self.transcript_width,
            self.language,
            &users,
        )
    }

    pub(super) fn update_user_message_hits(&mut self, offset: usize, top: u16) {
        self.user_message_hit_rows = self
            .user_message_controls
            .iter()
            .filter(|(line, _)| *line >= offset && *line < offset + self.viewport_height)
            .map(|(line, source)| (top.saturating_add((line - offset) as u16), *source))
            .collect();
    }

    pub(super) fn toggle_user_message_at(&mut self, y: u16) -> bool {
        let Some((_, source)) = self
            .user_message_hit_rows
            .iter()
            .find(|(line, _)| *line == y)
        else {
            return false;
        };
        let source = *source;
        let Some(row) = self
            .transcript
            .get(source)
            .filter(|row| row.starts_with("You: "))
        else {
            return false;
        };
        let collapsing = self.expanded_user_messages.get(&source) == Some(row);
        let mut offset = self.transcript_render_offset;
        if collapsing {
            let body_height = wrap_styled_text(
                colored_transcript_at_width(std::slice::from_ref(row), None, self.transcript_width),
                self.transcript_width,
            )
            .lines
            .len();
            if let Some((control, _)) = self
                .user_message_controls
                .iter()
                .find(|(_, index)| *index == source)
            {
                offset = offset.min(control.saturating_sub(body_height));
            }
            self.expanded_user_messages.remove(&source);
        } else {
            self.expanded_user_messages.insert(source, row.clone());
        }
        self.transcript_render_cache = None;
        self.chat_selection = None;
        self.user_message_hit_rows.clear();
        self.refresh_transcript_height();
        self.follow_bottom = false;
        self.scroll_from_bottom = self.max_scroll().saturating_sub(offset);
        true
    }
}
