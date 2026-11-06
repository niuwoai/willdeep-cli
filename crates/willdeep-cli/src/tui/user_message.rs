use super::narration::FoldedTranscript;
use super::*;

pub(super) const PREVIEW_LINES: usize = 8;

impl App {
    /// 普通 Enter 的本地反馈先准备好，调用方立刻绘制，再执行命令或提交 Runtime。
    pub(super) fn take_prompt_for_submission(&mut self) -> String {
        let prompt = self.take_submitted_input();
        if !narration::is_display_only_command(&prompt) {
            self.append_transcript(format!("You: {prompt}"));
            self.note_prompt_submitted(&prompt);
            self.scroll_to_bottom();
        }
        prompt
    }

    pub(super) fn render_user_transcript(
        &self,
        folded: &FoldedTranscript,
        labels: &[Option<String>],
        search: Option<&str>,
        end: usize,
    ) -> message_time::FoldedFrame {
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
        // 点开的工具行按记录位置认。折叠视图里同一行可能对应好几条记录，
        // 只有用户点开的那条才展开，其余的照旧收着。
        let tools = self
            .transcript
            .iter()
            .enumerate()
            .skip(floor)
            .filter(|(_, row)| narration::is_tool_display_row(row))
            .filter_map(|(source, row)| {
                let target = *folded.index_of.get(source - floor + shift)?;
                let visible = folded.rows.get(target)?;
                if visible != row {
                    return None;
                }
                // 行本身一直可点，但完整命令只在点开之后才画出来。
                // 记下的展开内容不管展开状态，收起时照样留着，下次点开还在。
                let expansion = self
                    .expanded_tool_rows
                    .contains(&source)
                    .then(|| self.tool_expansions.get(&source).cloned())
                    .flatten();
                Some((target, (source, expansion)))
            })
            .collect();
        message_time::render_with_user_folds(
            &folded.rows[..end],
            &labels[..end],
            search,
            self.transcript_width,
            self.language,
            &users,
            &tools,
        )
    }

    pub(super) fn update_user_message_hits(&mut self, offset: usize, top: u16) {
        self.user_message_hit_rows = self
            .user_message_controls
            .iter()
            .filter(|(line, _)| *line >= offset && *line < offset + self.viewport_height)
            .map(|(line, source)| (top.saturating_add((line - offset) as u16), *source))
            .collect();
        self.tool_row_hits = self
            .tool_row_controls
            .iter()
            .filter(|(line, _, _)| *line >= offset && *line < offset + self.viewport_height)
            .map(|(line, source, collapse)| {
                (
                    top.saturating_add((line - offset) as u16),
                    *source,
                    *collapse,
                )
            })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_submission_prepares_the_visible_message_and_clears_the_composer() {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert("show this before the Runtime responds");
        app.composer_expanded = true;
        app.follow_bottom = false;
        let prompt = app.take_prompt_for_submission();
        assert_eq!(prompt, "show this before the Runtime responds");
        assert!(app.input.is_empty());
        assert!(!app.composer_expanded);
        assert!(app.follow_bottom);
        assert_eq!(
            app.transcript.last().unwrap(),
            "You: show this before the Runtime responds"
        );
        assert_eq!(
            app.prompt_history_pending.back().map(String::as_str),
            Some(prompt.as_str())
        );
    }

    #[test]
    fn model_command_consumes_input_without_echoing_it_as_a_user_message() {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert("/model selected-model");
        assert_eq!(app.take_prompt_for_submission(), "/model selected-model");
        assert!(app.input.is_empty());
        assert!(app.transcript.is_empty());
    }
}
