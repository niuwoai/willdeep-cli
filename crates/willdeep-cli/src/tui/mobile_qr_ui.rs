use super::*;
mod geometry;
use geometry::{square_popup, square_qr};

pub(super) fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    app.mobile_qr_rect = Rect::default();
    let Some(qr) = &app.mobile_qr else { return };
    let cell = terminal::window_size()
        .ok()
        .filter(|size| {
            size.columns > 0
                && size.rows > 0
                && size.width >= size.columns
                && size.height >= size.rows
        })
        .map(|size| (size.width / size.columns, size.height / size.rows))
        .unwrap_or_else(|| app.media.font_size());
    let lines = square_qr(qr, cell);
    let columns = lines.first().map(|line| line.chars().count()).unwrap_or(0) as u16;
    let rows = lines.len() as u16;
    let (width, height) = square_popup(columns, rows, cell);
    let area = frame.area();
    let fits = width <= area.width && height <= area.height;
    let popup = centered_rect(width.min(area.width), height.min(area.height), area);
    app.mobile_qr_rect = popup;
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(app.language.text(
            "使用 WillDeep Mobile 扫码 · Esc 隐藏",
            "Scan with WillDeep Mobile · Esc hides",
            "WillDeep Mobile でスキャン · Esc で非表示",
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if !fits {
        frame.render_widget(Paragraph::new(app.language.text(
            "窗口太小，无法完整显示二维码。请放大窗口或缩小字号；Esc 隐藏。",
            "Window too small for the complete QR. Enlarge it or reduce the font size; Esc hides.",
            "QR 全体を表示できません。ウィンドウを拡大するか文字を小さくしてください。Esc で非表示。",
        )).wrap(Wrap { trim: false }), inner);
        return;
    }
    let qr_area = centered_rect(columns, rows, inner);
    frame.render_widget(Paragraph::new(lines.join("\n")), qr_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_window_shows_resize_hint_instead_of_a_partial_qr() {
        let mut app = App::new(Vec::new(), Language::En);
        app.mobile_qr = Some(crate::mobile::render_qr("local-test-only").unwrap());
        let backend = ratatui::backend::TestBackend::new(40, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Window too small"));
        assert!(
            !text
                .chars()
                .any(|character| matches!(character, '█' | '▀' | '▄'))
        );
    }
}
