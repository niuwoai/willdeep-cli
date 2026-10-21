use super::*;
use std::io::Write;

impl App {
    pub(super) fn composer_ready(&self) -> bool {
        !self.running
            && self.queued_prompts.is_empty()
            && self.approval.is_none()
            && self.question.is_none()
    }

    pub(super) fn composer_style(&self) -> Style {
        Style::default()
            .fg(Color::Cyan)
            .bg(if self.composer_ready() {
                COMPOSER_BACKGROUND
            } else {
                Color::Reset
            })
    }

    pub(super) fn composer_hint_style(&self) -> Style {
        let ui = self.ui;
        let mut style = Style::default().fg(ui.input_suggestion_color.ratatui());
        if ui.input_suggestion_italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if ui.input_suggestion_underline {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if !self.composer_ready() {
            style = style.add_modifier(Modifier::DIM);
        }
        style
    }

    pub(super) fn ring_turn_bell(&mut self, writer: &mut impl Write) -> io::Result<()> {
        if std::mem::take(&mut self.bell_pending) && self.composer_ready() {
            writer.write_all(b"\x07")?;
            writer.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_runtime_completion_ring_once_without_ringing_on_startup() {
        for runtime in [false, true] {
            let mut app = App::new(Vec::new(), Language::En);
            let mut bytes = Vec::new();
            app.ring_turn_bell(&mut bytes).unwrap();
            assert!(bytes.is_empty());
            app.begin_turn(runtime, "working".into());
            app.finish_turn();
            app.ring_turn_bell(&mut bytes).unwrap();
            app.finish_turn(); // 重复终态或快照不能重复提醒。
            app.ring_turn_bell(&mut bytes).unwrap();
            assert_eq!(bytes, b"\x07");
            app.begin_turn(runtime, "next".into());
            app.finish_turn();
            app.begin_turn(runtime, "immediate continuation".into());
            app.ring_turn_bell(&mut bytes).unwrap();
            assert_eq!(bytes, b"\x07");
        }
    }

    #[test]
    fn ready_background_covers_empty_cells_and_restores_when_work_resumes() {
        let mut app = App::new(Vec::new(), Language::En);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        let skills = SkillCatalog::default();
        app.input.insert("unsent draft");
        app.begin_turn(true, "working".into());
        app.finish_turn();
        assert_eq!(app.composer_style().fg, Some(Color::Cyan));
        assert_eq!(app.composer_style().bg, Some(Color::Black));
        draw(&mut terminal, &mut app, &skills).unwrap();
        let rect = app.prompt_rect;
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                assert_eq!(terminal.backend().buffer()[(x, y)].bg, COMPOSER_BACKGROUND);
            }
        }
        assert_eq!(app.input.text(), "unsent draft");
        app.begin_turn(true, "next".into());
        app.set_background_wait(true);
        draw(&mut terminal, &mut app, &skills).unwrap();
        let rect = app.prompt_rect;
        assert_eq!(
            terminal.backend().buffer()[(rect.x + 1, rect.bottom() - 2)].bg,
            Color::Reset
        );
        assert_eq!(app.input.text(), "unsent draft");
        assert!(!app.composer_ready());
    }

    #[test]
    fn queued_messages_suppress_the_ready_cue_and_bell() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "working".into());
        app.queued_prompts.push_back(QueuedPrompt {
            text: "queued follow-up".into(),
            attachments: Vec::new(),
        });
        app.finish_turn();
        let mut bytes = Vec::new();
        app.ring_turn_bell(&mut bytes).unwrap();
        assert!(bytes.is_empty());
        assert!(!app.composer_ready());
        assert_eq!(app.composer_style().bg, Some(Color::Reset));
        assert_eq!(app.queued_prompts.front().unwrap().text, "queued follow-up");
    }

    #[test]
    fn bell_flushes_the_terminal_output_once() {
        #[derive(Default)]
        struct Output {
            bytes: Vec<u8>,
            flushes: usize,
        }
        impl Write for Output {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }
        let mut app = App::new(Vec::new(), Language::En);
        let mut output = Output::default();
        app.begin_turn(false, "working".into());
        app.finish_turn();
        app.ring_turn_bell(&mut output).unwrap();
        app.ring_turn_bell(&mut output).unwrap();
        assert_eq!(output.bytes, b"\x07");
        assert_eq!(output.flushes, 1);
    }

    #[test]
    fn input_suggestion_style_uses_configured_color_and_modifiers() {
        let mut app = App::new(Vec::new(), Language::En);
        app.ui.input_suggestion_italic = true;
        app.ui.input_suggestion_underline = true;
        let style = app.composer_hint_style();
        assert_eq!(style.fg, Some(Color::Gray));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));

        app.ui.input_suggestion_color = crate::config::InputSuggestionColor::DarkGray;
        assert_eq!(app.composer_hint_style().fg, Some(Color::DarkGray));
    }
}
