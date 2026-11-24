//! 粘贴 Finder 等复制的图片路径（#197）。

use super::*;
use std::path::Path;

const PASTED_IMAGE_SUFFIXES: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".gif"];

impl App {
    /// Finder 等复制出来的路径（含 `file://`）优先收成图片附件。
    pub(super) fn try_attach_pasted_image_paths(&mut self, value: &str) -> Option<usize> {
        const MAX_IMAGE_PATHS: usize = 8;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lines: Vec<&str> = trimmed.lines().map(str::trim).collect();
        let candidates: Vec<PathBuf> = lines
            .iter()
            .filter_map(|line| pasted_path_candidate(line))
            .collect();
        if candidates.is_empty() || candidates.len() > MAX_IMAGE_PATHS {
            return None;
        }
        if lines
            .iter()
            .any(|line| !line.is_empty() && pasted_path_candidate(line).is_none())
        {
            return None;
        }
        let workspace = self.workspace.clone()?;
        let mut attached = 0usize;
        for path in candidates {
            let resolved = resolve_paste_image_path(&path, workspace.as_path())?;
            let draft = draft_attachment_from_image_file(&resolved)?;
            if self.append_draft_attachment(draft) {
                attached += 1;
            }
        }
        if attached > 0 { Some(attached) } else { None }
    }
}

fn pasted_path_candidate(line: &str) -> Option<PathBuf> {
    let mut raw = line.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("file://") {
        raw = rest;
        if raw.starts_with("localhost/") {
            raw = &raw["localhost".len()..];
        }
        raw = raw.trim_start_matches('/');
    }
    let path = Path::new(raw);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    let dotted = format!(".{ext}");
    if !PASTED_IMAGE_SUFFIXES.contains(&dotted.as_str()) {
        return None;
    }
    Some(path.to_path_buf())
}

fn resolve_paste_image_path(path: &Path, workspace: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        path.exists().then(|| path.to_path_buf())
    } else if workspace.join(path).exists() {
        Some(workspace.join(path))
    } else {
        path.exists().then(|| path.to_path_buf())
    }
}

fn draft_attachment_from_image_file(path: &Path) -> Option<DraftAttachment> {
    const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
    const MAX_IMAGE_PIXELS: u64 = 100_000_000;
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
        return None;
    }
    let format = image::guess_format(&bytes).ok()?;
    let media_type = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Gif => "image/gif",
        _ => return None,
    };
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if width == 0
        || height == 0
        || u64::from(width).saturating_mul(u64::from(height)) > MAX_IMAGE_PIXELS
    {
        return None;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("image.png")
        .to_owned();
    Some(DraftAttachment {
        id: uuid::Uuid::new_v4(),
        message: MessageAttachment::Image {
            name,
            media_type: media_type.to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            width,
            height,
        },
    })
}
