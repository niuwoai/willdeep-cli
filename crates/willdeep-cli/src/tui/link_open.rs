//! MuseTerm 聊天区：含可打开 HTTP(S) 链接的行右上角显示 ↗，点击在系统浏览器打开（#190）。

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LinkOnRow {
    pub(super) row: usize,
    pub(super) url: String,
}

#[derive(Default)]
pub(super) struct LinkOpenState {
    hosted: bool,
    links: Vec<LinkOnRow>,
    badges: Vec<(Rect, usize)>,
}

pub(super) enum LinkOpenAction {
    Open(String),
}

impl LinkOpenState {
    pub(super) fn new(hosted: bool) -> Self {
        Self {
            hosted,
            ..Self::default()
        }
    }

    pub(super) fn refresh(&mut self, rows: &[String]) {
        self.links.clear();
        if !self.hosted {
            return;
        }
        for (row, line) in rows.iter().enumerate() {
            if let Some(url) = first_http_url(line) {
                self.links.push(LinkOnRow { row, url });
            }
        }
    }

    fn badge_label(language: Language) -> String {
        let core = language.text("↗ 打开", "↗ Open", "↗ 開く");
        format!("╭ {core} ╮")
    }

    fn badge_width(label: &str) -> u16 {
        UnicodeWidthStr::width(label).min(u16::MAX as usize) as u16
    }

    fn badge_rect(app: &App, row: usize, label_width: u16) -> Option<Rect> {
        let content = app.transcript_content;
        const MARGIN: u16 = 1;
        let width = label_width;
        if content.width < width.saturating_add(MARGIN) || content.height == 0 {
            return None;
        }
        let offset = app.transcript_render_offset;
        let viewport = app.viewport_height.max(1);
        if row < offset || row >= offset.saturating_add(viewport) {
            return None;
        }
        let y = content.y + (row - offset) as u16;
        if y >= content.bottom() {
            return None;
        }
        let right_limit = content.right().saturating_sub(MARGIN);
        let x = right_limit.saturating_sub(width);
        if x < content.x {
            return None;
        }
        Some(Rect {
            x,
            y,
            width,
            height: 1,
        })
    }
}

pub(super) fn draw(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    app.link_open.badges.clear();
    if !app.link_open.hosted || app.link_open.links.is_empty() {
        return;
    }
    let label = LinkOpenState::badge_label(app.language);
    let label_width = LinkOpenState::badge_width(&label);
    let style = Style::default()
        .fg(Color::Rgb(255, 255, 255))
        .bg(Color::Rgb(29, 78, 216));
    let buffer = frame.buffer_mut();
    for (index, link) in app.link_open.links.iter().enumerate() {
        let Some(rect) = LinkOpenState::badge_rect(app, link.row, label_width) else {
            continue;
        };
        buffer.set_string(rect.x, rect.y, label.as_str(), style);
        app.link_open.badges.push((rect, index));
    }
}

impl App {
    pub(super) fn handle_link_open_mouse(&mut self, mouse: MouseEvent) -> Option<LinkOpenAction> {
        if !self.link_open.hosted {
            return None;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            for (rect, index) in &self.link_open.badges {
                if rect.contains((mouse.column, mouse.row).into())
                    && let Some(link) = self.link_open.links.get(*index)
                {
                    self.pending_chat_click = None;
                    return Some(LinkOpenAction::Open(link.url.clone()));
                }
            }
        }
        None
    }
}

fn first_http_url(line: &str) -> Option<String> {
    if let Some(url) = markdown_link_target(line) {
        return Some(url);
    }
    for word in line.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            matches!(
                c,
                '<' | '>' | '(' | ')' | '[' | ']' | ',' | ';' | '.' | '!' | '?' | '\'' | '"'
            )
        });
        if openable_http(trimmed) {
            return Some(trimmed.to_owned());
        }
    }
    None
}

fn markdown_link_target(value: &str) -> Option<String> {
    let start = value.find("](")? + 2;
    let end = value[start..].find(')')? + start;
    let target = value.get(start..end)?.trim();
    openable_http(target).then(|| target.to_owned())
}

fn openable_http(value: &str) -> bool {
    (value.starts_with("https://") || value.starts_with("http://")) && !value.contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_markdown_target_over_bare_url() {
        assert_eq!(
            first_http_url("See [Docs](https://a.example) https://b.example"),
            Some("https://a.example".to_owned())
        );
    }

    #[test]
    fn rejects_urls_with_credentials() {
        assert!(first_http_url("https://user:secret@x.com").is_none());
    }

    #[test]
    fn badge_label_uses_pill_corners_and_open_text() {
        let label = LinkOpenState::badge_label(Language::ZhCn);
        assert!(label.starts_with('╭'));
        assert!(label.ends_with('╮'));
        assert!(label.contains("打开"));
    }
}
