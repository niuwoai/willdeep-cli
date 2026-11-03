//! 首屏吉祥物：深潜鲸。
//!
//! 同一份像素网格同时生成两种东西——进 transcript 的纯文本行，和带真彩色的
//! `Line`。两边的行数与字符一一对应，所以拖选、复制、搜索拿到的就是屏幕上
//! 看到的字形，不会因为「画的时候换了样子」而对不上。
//!
//! 聊天区窄于鲸鱼宽度时换成小号：两个变体的**行数相同**，只缩宽度。行数一致
//! 很重要——选区按行号定位，行数一变，选中的字就会和屏幕错开。
//!
//! 终端不支持真彩色（`NO_COLOR` 或 `TERM=dumb`）时退回单一颜色，形状不变。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::sync::OnceLock;

/// 宽版鲸鱼像素网格：每个字符一个像素，上下两行像素拼成一行终端单元格。
/// `.` 是透明，其余标记见 `pixel`。
const ART: [&str; 14] = [
    "........................",
    "............BB..........",
    "............BB..........",
    ".....ss....BBBB.........",
    "....BBBBBBBBBBBBB.......",
    "...BBBBBBBBBBBBBBBB.....",
    "..bbbbbbbbbbbbbbeebb....",
    "..bbbbbbbbbbbbbbeebbb...",
    "..bbbbbbbbbbbbbbbbbbbb..",
    "..bbbbbbbbbbbbbbbbbbbbb.",
    "...lllllllllllllllllllll",
    "....llllllllllllllllll..",
    ".....llllllllllllll.....",
    "........llllllll........",
];

/// 窄版：同样的 14 行像素，只缩到 12 列，仍然是 7 行终端单元格。
const SMALL_ART: [&str; 14] = [
    "............",
    ".....ss.....",
    ".....ss.....",
    "....BBBB....",
    "..BBBBBBBB..",
    ".BBBBBBBBBB.",
    "BBBeebbbbbbb",
    "BBBeebbbbbbb",
    "bbbbbbbbbbbb",
    "bbbbbbbbbbbb",
    ".llllllllll.",
    "..llllllll..",
    "...llllll...",
    "....llll....",
];

/// 产品名那一行。复制出来仍然是这几个字。
pub(super) const NAME: &str = "WILLDEEP";

const COLUMNS: usize = 24;

fn pixel(ch: char) -> Option<Color> {
    let (r, g, b) = match ch {
        // 背脊：深一点，和水柱拉开层次
        'B' => (26, 78, 158),
        'b' => (38, 112, 196),
        // 肚皮
        'l' => (156, 214, 246),
        // 眼睛
        'e' => (12, 22, 38),
        // 水柱
        's' => (126, 232, 250),
        _ => return None,
    };
    Some(Color::Rgb(r, g, b))
}

/// 只认 `NO_COLOR` 和 `TERM=dumb` 这两种明确的「别上色」信号；
/// 真彩支持与否交给终端自己降级，猜错的代价比不猜大。
fn color_supported() -> bool {
    std::env::var_os("NO_COLOR").is_none()
        && !matches!(std::env::var("TERM").as_deref(), Ok("dumb"))
}

fn pixel_at(art: &[&str], row: usize, column: usize) -> Option<Color> {
    art.get(row)?.chars().nth(column).and_then(pixel)
}

/// 上下两个像素拼成一个单元格：都空是空格，都同色是整块，
/// 只有一边是半块，两边不同色时用上半块加背景。
fn cell(top: Option<Color>, bottom: Option<Color>) -> (char, Style) {
    match (top, bottom) {
        (None, None) => (' ', Style::default()),
        (Some(top), None) => ('▀', Style::default().fg(top)),
        (None, Some(bottom)) => ('▄', Style::default().fg(bottom)),
        (Some(top), Some(bottom)) if top == bottom => ('█', Style::default().fg(top)),
        (Some(top), Some(bottom)) => ('▀', Style::default().fg(top).bg(bottom)),
    }
}

fn rows_of(art: &[&str], columns: usize) -> Vec<String> {
    (0..art.len() / 2)
        .map(|line| {
            (0..columns)
                .map(|column| {
                    cell(
                        pixel_at(art, line * 2, column),
                        pixel_at(art, line * 2 + 1, column),
                    )
                    .0
                })
                .collect()
        })
        .collect()
}

fn plain_rows() -> &'static [String] {
    static ROWS: OnceLock<Vec<String>> = OnceLock::new();
    ROWS.get_or_init(|| rows_of(&ART, COLUMNS))
}

fn small_rows() -> &'static [String] {
    static ROWS: OnceLock<Vec<String>> = OnceLock::new();
    ROWS.get_or_init(|| rows_of(&SMALL_ART, SMALL_ART[0].chars().count()))
}

/// 进 transcript 的纯文本行。始终是宽版：文本是会话开始就定下来的，
/// 之后终端变窄只会改变**画法**，不会改写记录，所以行数与行号保持不变。
pub(super) fn art_rows() -> &'static [String] {
    plain_rows()
}

/// 这一行是不是吉祥物的一部分；是的话按当前宽度给出版本带颜色的画法。
/// `width` 是聊天区可用的内宽：放得下宽版就画宽版，否则换小号。
pub(super) fn styled_line(row: &str, width: usize) -> Option<Line<'static>> {
    let line = plain_rows().iter().position(|plain| plain == row)?;
    let (art, plain): (&[&str], &[String]) = if width >= COLUMNS {
        (&ART, plain_rows())
    } else {
        (&SMALL_ART, small_rows())
    };
    if !color_supported() {
        return Some(Line::styled(
            plain[line].clone(),
            Style::default().fg(Color::Cyan),
        ));
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for column in 0..plain[line].chars().count() {
        let (glyph, style) = cell(
            pixel_at(art, line * 2, column),
            pixel_at(art, line * 2 + 1, column),
        );
        let text = glyph.to_string();
        // 相邻同款合并：一行一个 Span 太碎，终端重绘也更慢
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(&text),
            _ => spans.push(Span::styled(text, style)),
        }
    }
    Some(Line::from(spans))
}

/// 产品名那一行：`WILLDEEP  v0.1.2`，名字加粗青色，版本灰。
pub(super) fn name_line(version: &str) -> String {
    format!("{NAME}  {version}")
}

pub(super) fn styled_name_line(row: &str) -> Option<Line<'static>> {
    let version = row.strip_prefix(NAME)?.strip_prefix("  ")?;
    if version.is_empty() {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(
            NAME.to_owned(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {version}"), Style::default().fg(Color::DarkGray)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_art_row_is_the_same_width_and_only_uses_known_pixels() {
        for (index, row) in ART.iter().enumerate() {
            assert_eq!(row.chars().count(), COLUMNS, "row {index} width");
            assert!(
                row.chars().all(|ch| ch == '.' || pixel(ch).is_some()),
                "row {index} has an unknown pixel"
            );
        }
        assert_eq!(ART.len() % 2, 0, "pixel rows must pair into cells");
    }

    #[test]
    fn plain_rows_keep_the_pixel_width_and_survive_copying() {
        let rows = art_rows();
        assert_eq!(rows.len(), ART.len() / 2);
        for row in rows {
            assert_eq!(row.chars().count(), COLUMNS);
            assert!(row.chars().all(|ch| matches!(ch, ' ' | '▀' | '▄' | '█')));
        }
    }

    /// 两个变体行数必须相同：选区按行号定位，行数一变，选中的字就和屏幕错开。
    #[test]
    fn the_small_variant_keeps_the_row_count() {
        assert_eq!(SMALL_ART.len(), ART.len());
        let small = rows_of(&SMALL_ART, SMALL_ART[0].chars().count());
        assert_eq!(small.len(), art_rows().len());
        assert!(
            small
                .iter()
                .all(|row| row.chars().count() == SMALL_ART[0].chars().count())
        );
        assert!(
            SMALL_ART[0].chars().count() < COLUMNS,
            "the small one must be narrower"
        );
    }

    /// 宽屏画宽版、窄屏画小号；两种情况下行数一致，宽度跟着聊天区走。
    #[test]
    fn a_narrow_chat_area_draws_the_small_whale_without_folding() {
        for (width, expected) in [(COLUMNS, COLUMNS), (COLUMNS - 1, 12), (8, 12)] {
            let lines: Vec<Line<'static>> = art_rows()
                .iter()
                .map(|row| styled_line(row, width).expect("mascot row"))
                .collect();
            assert_eq!(lines.len(), art_rows().len(), "row count at width {width}");
            for line in &lines {
                let text: String = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
                assert_eq!(
                    text.chars().count(),
                    expected,
                    "width {width} should render a {expected}-column whale"
                );
            }
        }
    }

    #[test]
    fn the_same_row_is_recognised_for_styling() {
        for row in art_rows() {
            assert!(
                styled_line(row, COLUMNS).is_some(),
                "{row:?} should be styled"
            );
        }
        assert!(styled_line("not part of the mascot", COLUMNS).is_none());
        assert!(styled_name_line(&name_line("v0.1.0")).is_some());
        assert!(styled_name_line("WILLDEEP").is_none());
    }

    #[test]
    fn the_silhouette_is_not_an_empty_rectangle() {
        let drawn = art_rows()
            .iter()
            .map(|row| row.chars().filter(|ch| *ch != ' ').count())
            .sum::<usize>();
        assert!(drawn > 40, "the mascot should actually draw something");
    }

    /// 两种颜色落在同一个单元格时，下半部分必须变成背景，否则那块就是纯色，
    /// 鲸鱼的背脊和肚皮会糊成一片。
    #[test]
    fn a_two_tone_cell_keeps_both_colours() {
        let top = Color::Rgb(26, 78, 158);
        let bottom = Color::Rgb(156, 214, 246);
        let (glyph, style) = cell(Some(top), Some(bottom));
        assert_eq!(glyph, '▀');
        assert_eq!(style.fg, Some(top));
        assert_eq!(style.bg, Some(bottom));

        // 同色整块只用前景，不叠一层背景
        let (glyph, style) = cell(Some(top), Some(top));
        assert_eq!(glyph, '█');
        assert_eq!(style.bg, None);
    }
}
