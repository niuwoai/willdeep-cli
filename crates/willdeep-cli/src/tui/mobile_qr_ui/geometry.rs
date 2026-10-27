/// A terminal cell need not have a 1:2 pixel aspect ratio. Resample the QR's
/// half-block grid to square pixels, preserving the quiet zone on every side.
pub(super) fn square_qr(qr: &str, cell: (u16, u16)) -> Vec<String> {
    let source: Vec<Vec<char>> = qr.lines().map(|line| line.chars().collect()).collect();
    let side = source.iter().map(Vec::len).max().unwrap_or(0);
    if side == 0 {
        return Vec::new();
    }
    let (cw, ch) = (usize::from(cell.0.max(1)), usize::from(cell.1.max(1)));
    let module_pixels = cw.max(ch.div_ceil(2));
    let pixels = side * module_pixels;
    let columns = pixels.div_ceil(cw);
    let rows = pixels.div_ceil(ch);
    (0..rows)
        .map(|row| {
            (0..columns)
                .map(|column| {
                    let x = column * side / columns;
                    let top = row * 2 * side / (rows * 2);
                    let bottom = (row * 2 + 1) * side / (rows * 2);
                    match (filled(&source, x, top), filled(&source, x, bottom)) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    }
                })
                .collect()
        })
        .collect()
}

fn filled(source: &[Vec<char>], x: usize, y: usize) -> bool {
    let character = source
        .get(y / 2)
        .and_then(|row| row.get(x))
        .copied()
        .unwrap_or(' ');
    character == '█' || character == if y.is_multiple_of(2) { '▀' } else { '▄' }
}

pub(super) fn square_popup(columns: u16, rows: u16, cell: (u16, u16)) -> (u16, u16) {
    let (cw, ch) = (u32::from(cell.0.max(1)), u32::from(cell.1.max(1)));
    let pixels = (u32::from(columns) + 2) * cw;
    let pixels = pixels.max((u32::from(rows) + 2) * ch);
    (pixels.div_ceil(cw) as u16, pixels.div_ceil(ch) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_qr() -> String {
        qrcode::QrCode::with_error_correction_level(b"local-test-only", qrcode::EcLevel::L)
            .unwrap()
            .render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build()
    }

    #[test]
    fn qr_and_frame_are_physically_square_for_tall_and_wide_cells() {
        let qr = test_qr();
        for cell in [(10, 20), (17, 42), (12, 18)] {
            let lines = square_qr(&qr, cell);
            let columns = lines[0].chars().count() as u16;
            let rows = lines.len() as u16;
            let pixel_width = i32::from(columns) * i32::from(cell.0);
            let pixel_height = i32::from(rows) * i32::from(cell.1);
            assert!((pixel_width - pixel_height).abs() < i32::from(cell.0.max(cell.1)));
            let (width, height) = square_popup(columns, rows, cell);
            assert!(
                (i32::from(width) * i32::from(cell.0) - i32::from(height) * i32::from(cell.1))
                    .abs()
                    < i32::from(cell.0.max(cell.1))
            );
            assert!(width >= columns + 2 && height >= rows + 2);
            assert!(lines.first().unwrap().trim().is_empty());
            assert!(lines.last().unwrap().trim().is_empty());
            if let Some(dir) = std::env::var_os("WILLDEEP_QR_QA_DIR") {
                save_preview(&lines, cell, std::path::Path::new(&dir));
            }
        }
    }

    #[test]
    fn normal_cells_preserve_the_original_modules_and_quiet_zone() {
        let qr = test_qr();
        let original: Vec<Vec<char>> = qr.lines().map(|line| line.chars().collect()).collect();
        let scaled: Vec<Vec<char>> = square_qr(&qr, (10, 20))
            .iter()
            .map(|line| line.chars().collect())
            .collect();
        let side = original[0].len();
        for y in 0..side {
            for x in 0..side {
                // Every source module is still represented in the output.
                let target_y = (y * scaled.len() * 2).div_ceil(side);
                assert_eq!(filled(&original, x, y), filled(&scaled, x, target_y));
            }
        }
    }

    fn save_preview(lines: &[String], cell: (u16, u16), dir: &std::path::Path) {
        let columns = lines[0].chars().count() as u32;
        let width = columns * u32::from(cell.0);
        let height = lines.len() as u32 * u32::from(cell.1);
        let source: Vec<Vec<char>> = lines.iter().map(|line| line.chars().collect()).collect();
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            let column = x / u32::from(cell.0);
            let row = y / u32::from(cell.1);
            let half = u32::from(y % u32::from(cell.1) >= u32::from(cell.1).div_ceil(2));
            image::Rgb(
                if filled(&source, column as usize, (row * 2 + half) as usize) {
                    [248, 248, 242]
                } else {
                    [57, 58, 72]
                },
            )
        });
        std::fs::create_dir_all(dir).unwrap();
        image
            .save(dir.join(format!("qr-{}x{}.png", cell.0, cell.1)))
            .unwrap();
    }
}
