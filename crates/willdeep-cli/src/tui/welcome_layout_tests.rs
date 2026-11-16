use super::*;
use ratatui::backend::TestBackend;

#[test]
fn complete_message_rendering_keeps_whale_rows_intact_at_every_narrow_width() {
    let original = banner::art_rows().join("\n");
    for width in 1..=26 {
        let rendered = message_time::render_with_times(
            std::slice::from_ref(&original),
            &[None],
            None,
            width,
            Language::En,
        );
        assert_eq!(
            rendered.lines.len(),
            banner::art_rows().len(),
            "width {width}"
        );
        assert!(rendered.lines.iter().all(|line| line.width() <= width));
        let expected = banner::art_rows()
            .iter()
            .map(|row| banner::styled_line(row, width).unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(text_rows(&rendered), expected, "width {width}");
    }
}

#[test]
fn both_layouts_keep_the_whale_intact_across_resize_and_cache_reuse() {
    let original = welcome_message(std::path::Path::new("/workspace"), Language::En);
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        let mut app = App::new(vec![original.clone()], Language::En);
        app.ui.mode = mode;
        for columns in [26u16, 25, 14, 13, 10, 3, 13, 26] {
            // Leave enough height for a greeting wrapped into single-column lines.
            let mut terminal = Terminal::new(TestBackend::new(columns, 512)).unwrap();
            for _ in 0..2 {
                draw(
                    &mut terminal,
                    &mut app,
                    &SkillCatalog::default(),
                    &quiet_test_session(),
                )
                .unwrap();
                let width = usize::from(columns - 2);
                assert_eq!(app.transcript_width, width);
                assert_eq!(app.transcript_render_offset, 0, "{mode:?} at {columns}");
                let expected = banner::art_rows()
                    .iter()
                    .map(|row| banner::styled_line(row, width).unwrap())
                    .collect::<Vec<_>>();
                for (row, line) in expected.iter().enumerate() {
                    assert_eq!(app.transcript_rows[row], line.to_string());
                    for (column, glyph) in line.to_string().chars().enumerate() {
                        let cell = &terminal.backend().buffer()[(
                            app.transcript_content.x + column as u16,
                            app.transcript_content.y + row as u16,
                        )];
                        assert_eq!(
                            cell.symbol(),
                            glyph.to_string(),
                            "{mode:?}: {columns} columns, whale row {row}, column {column}"
                        );
                    }
                }
                assert!(app.transcript_rows[expected.len()].is_empty());
                assert_eq!(app.transcript.as_slice(), std::slice::from_ref(&original));
                if columns == 26 {
                    assert_eq!(&app.transcript_rows[..expected.len()], banner::art_rows());
                }
            }
        }
    }
}
