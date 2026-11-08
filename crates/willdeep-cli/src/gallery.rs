//! The MuseTerm image column, spoken over the private OSC 7771 sequence.
//!
//! MuseTerm draws the thumbnails. This process owns the attachments, so it announces them and
//! answers when the terminal reports a paste or a click on a delete icon. The terminal only sends
//! those bytes after it has seen `hello`, and `hello` is only written once this filter is
//! installed in front of the terminal's own input reader.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

use base64::Engine;
use uuid::Uuid;
use willdeep_core::MessageAttachment;

const OSC: &[u8] = b"\x1b]7771;";
const VERSION: &str = "v=1";
const MAX_FRAME: usize = 120_000;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// One attachment as the terminal needs to hear about it. The pixels travel separately, and only
/// when this process produced them itself.
pub(crate) struct GalleryImage {
    pub id: String,
    pub name: String,
    pub mutable: bool,
}

/// What the terminal asked for, pulled out of the input stream before any key parser sees it.
#[derive(Debug)]
pub(crate) enum GalleryRequest {
    Paste { nonce: String, png: Vec<u8> },
    Delete { id: String },
}

/// Writes one frame. The terminal swallows this OSC number, so it never lands on the screen.
pub(crate) fn announce(out: &mut impl Write, operation: &str, body: &str) -> io::Result<()> {
    // BEL, not ST. A base64 payload can contain 0x5c, which would end an ESC ST frame early.
    write!(out, "\x1b]7771;{VERSION};{operation};{body}\x07")
}

/// The list the column shows. An empty list makes the terminal fold the column away.
pub(crate) fn announce_list(out: &mut impl Write, images: &[GalleryImage]) -> io::Result<()> {
    let rows: Vec<_> = images
        .iter()
        .filter(|image| is_identifier(&image.id))
        .map(|image| serde_json::json!({"id": image.id, "name": image.name, "mutable": image.mutable}))
        .collect();
    announce(
        out,
        "list",
        &serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_owned()),
    )
}

/// Tells the terminal a paste became a draft attachment.
pub(crate) fn acknowledge(out: &mut impl Write, nonce: &str, id: Uuid) -> io::Result<()> {
    announce(out, "ack", &format!("{nonce};{id}"))
}

/// Tells the terminal a paste was refused, so it can drop the thumbnail it showed early.
pub(crate) fn reject(out: &mut impl Write, nonce: &str, reason: &str) -> io::Result<()> {
    let reason: String = reason
        .chars()
        .filter(|ch| *ch >= ' ' && *ch != ';')
        .take(80)
        .collect();
    announce(out, "reject", &format!("{nonce};{reason}"))
}

/// Confirms that a draft attachment is gone.
pub(crate) fn confirm_removed(out: &mut impl Write, id: &str) -> io::Result<()> {
    announce(out, "remove", id)
}

/// Sends an image this process produced, such as a generated picture, so the column can show it.
#[allow(dead_code)]
pub(crate) fn announce_blob(out: &mut impl Write, id: &str, png: &[u8]) -> io::Result<()> {
    if !is_identifier(id) || png.is_empty() || png.len() > MAX_IMAGE_BYTES {
        return Ok(());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let piece_budget = 40_000;
    let total = encoded.len().div_ceil(piece_budget).max(1);
    for (index, piece) in encoded.as_bytes().chunks(piece_budget).enumerate() {
        let piece = std::str::from_utf8(piece).unwrap_or_default();
        announce(out, "blob", &format!("{id};{index};{total};{piece}"))?;
    }
    Ok(())
}

fn is_identifier(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Bytes of one paste that have arrived so far.
struct PartialPaste {
    nonce: String,
    total: usize,
    pieces: Vec<Option<String>>,
}

/// Pulls gallery frames out of the terminal's input and leaves every other byte untouched.
pub(crate) struct GalleryFilter {
    pending: Vec<u8>,
    pastes: Vec<PartialPaste>,
}

impl GalleryFilter {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::new(),
            pastes: Vec::new(),
        }
    }

    /// Returns the bytes a key parser should see, plus any request that became complete.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> (Vec<u8>, Vec<GalleryRequest>) {
        self.pending.extend_from_slice(bytes);
        let mut plain = Vec::new();
        let mut requests = Vec::new();
        while !self.pending.is_empty() {
            // Only an ESC can begin the sequence. Holding back everything that merely lacks the
            // marker would swallow the last few keystrokes of every read.
            let Some(escape) = self.pending.iter().position(|byte| *byte == 0x1b) else {
                plain.append(&mut self.pending);
                break;
            };
            if escape > 0 {
                plain.extend(self.pending.drain(..escape));
            }
            if self.pending.len() < OSC.len() {
                break;
            }
            if !self.pending.starts_with(OSC) {
                plain.push(self.pending.remove(0));
                continue;
            }
            let Some(end) = frame_end(&self.pending) else {
                if self.pending.len() > MAX_FRAME {
                    plain.extend(self.pending.drain(..OSC.len()));
                }
                break;
            };
            let frame: Vec<u8> = self.pending.drain(..end).collect();
            if let Some(request) = self.decode(&frame) {
                requests.push(request);
            }
        }
        (plain, requests)
    }

    fn decode(&mut self, frame: &[u8]) -> Option<GalleryRequest> {
        let body = frame.strip_prefix(OSC)?.strip_suffix(b"\x07")?;
        let text = std::str::from_utf8(body).ok()?;
        let mut fields = text.split(';');
        if fields.next() != Some(VERSION) {
            return None;
        }
        match fields.next() {
            // `nonce`, `index` and `total` are separate fields. Joining them back into `lead` and
            // splitting again used to see only the nonce, so every paste piece was discarded.
            Some("drop") => self.decode_drop(&mut fields),
            Some("delete") => {
                let id = fields.next()?;
                is_identifier(id).then(|| GalleryRequest::Delete { id: id.to_owned() })
            }
            _ => None,
        }
    }

    fn decode_drop<'a>(
        &mut self,
        fields: &mut impl Iterator<Item = &'a str>,
    ) -> Option<GalleryRequest> {
        let nonce = fields.next()?;
        let index: usize = fields.next()?.parse().ok()?;
        let total: usize = fields.next()?.parse().ok()?;
        let piece = fields.collect::<Vec<_>>().join(";");
        if !is_identifier(nonce) || piece.is_empty() || total == 0 || total > 999 || index >= total
        {
            return None;
        }
        if !self.pastes.iter().any(|paste| paste.nonce == nonce) {
            self.pastes.push(PartialPaste {
                nonce: nonce.to_owned(),
                total,
                pieces: vec![None; total],
            });
        }
        let paste = self.pastes.iter_mut().find(|paste| paste.nonce == nonce)?;
        if paste.total != total || paste.pieces[index].is_some() {
            return None;
        }
        paste.pieces[index] = Some(piece.to_owned());
        if paste.pieces.iter().any(Option::is_none) {
            return None;
        }
        let joined: String = paste
            .pieces
            .iter()
            .filter_map(|piece| piece.as_deref())
            .collect();
        let nonce = paste.nonce.clone();
        self.pastes.retain(|paste| paste.nonce != nonce);
        let png = base64::engine::general_purpose::STANDARD
            .decode(joined)
            .ok()?;
        if png.is_empty() || png.len() > MAX_IMAGE_BYTES {
            return None;
        }
        Some(GalleryRequest::Paste { nonce, png })
    }
}

fn frame_end(bytes: &[u8]) -> Option<usize> {
    // Only BEL. The string terminator's second byte also occurs inside base64.
    bytes
        .iter()
        .position(|byte| *byte == 0x07)
        .map(|index| index + 1)
}

/// Reads the terminal's input from the front of a pipe and writes everything that is not a
/// gallery frame to the back, where the terminal's own reader is attached.
///
/// The two ends trade places: the program's standard input becomes the filtered end, and this
/// reads the original. A program that never installs the filter never receives gallery bytes,
/// because the terminal waits for `hello` first.
pub(crate) struct GalleryInput {
    _owned: std::fs::File,
}

impl GalleryInput {
    pub(crate) fn install() -> io::Result<(Self, GalleryForwarder)> {
        let mut ends = [0; 2];
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let source = unsafe { std::fs::File::from_raw_fd(ends[0]) };
        let sink = unsafe { std::fs::File::from_raw_fd(ends[1]) };
        let saved = duplicate(io::stdin().as_raw_fd())?;
        if unsafe { libc::dup2(source.as_raw_fd(), libc::STDIN_FILENO) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((
            Self { _owned: source },
            GalleryForwarder {
                input: unsafe { std::fs::File::from_raw_fd(saved) },
                sink,
                filter: GalleryFilter::new(),
                waiting: std::collections::VecDeque::new(),
            },
        ))
    }
}

pub(crate) struct GalleryForwarder {
    input: std::fs::File,
    sink: std::fs::File,
    filter: GalleryFilter,
    /// Requests already decoded but not yet handed out. One read can finish several frames, and
    /// dropping all but one would lose a paste or a delete.
    waiting: std::collections::VecDeque<GalleryRequest>,
}

impl GalleryForwarder {
    /// Blocks until the program produces a complete request or the input closes.
    pub(crate) fn recv(&mut self) -> io::Result<Option<GalleryRequest>> {
        if let Some(request) = self.waiting.pop_front() {
            return Ok(Some(request));
        }
        let mut buffer = [0; 4096];
        loop {
            let read = self.input.read(&mut buffer)?;
            if read == 0 {
                return Ok(None);
            }
            let (plain, requests) = self.filter.push(&buffer[..read]);
            self.sink.write_all(&plain)?;
            self.waiting.extend(requests);
            if let Some(request) = self.waiting.pop_front() {
                return Ok(Some(request));
            }
        }
    }
}

fn duplicate(fd: RawFd) -> io::Result<RawFd> {
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(copy)
    }
}

/// The images currently on screen, in the order the column should show them.
pub(crate) fn gallery_images(drafts: &[crate::editor::DraftAttachment]) -> Vec<GalleryImage> {
    drafts
        .iter()
        .filter_map(|attachment| match &attachment.message {
            MessageAttachment::Image { name, .. } => Some(GalleryImage {
                id: attachment.id.to_string(),
                name: name.clone(),
                mutable: true,
            }),
            MessageAttachment::Text { .. } => None,
        })
        .collect()
}

/// Turns a pasted PNG into a draft image. The dimensions come from the file, not from anyone's
/// word, and a file that is not a PNG is refused.
pub(crate) fn attachment_from_png(png: &[u8]) -> Option<crate::editor::DraftAttachment> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    Some(crate::editor::DraftAttachment {
        id: Uuid::new_v4(),
        message: MessageAttachment::Image {
            name: "clipboard.png".to_owned(),
            media_type: "image/png".to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(png),
            width: image.width(),
            height: image.height(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through_untouched() {
        let mut filter = GalleryFilter::new();
        let (plain, requests) = filter.push(b"hello \x1b[A there");
        assert_eq!(plain, b"hello \x1b[A there");
        assert!(requests.is_empty());
    }

    #[test]
    fn a_delete_between_keystrokes_never_reaches_the_parser() {
        let mut filter = GalleryFilter::new();
        let (plain, requests) =
            filter.push(b"ab\x1b]7771;v=1;delete;11111111-2222-3333-4444-555555555555\x07c");
        assert_eq!(plain, b"abc");
        match requests.as_slice() {
            [GalleryRequest::Delete { id }] => {
                assert_eq!(id, "11111111-2222-3333-4444-555555555555")
            }
            other => panic!("expected one delete, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_split_across_reads_is_held_back() {
        let mut filter = GalleryFilter::new();
        let (plain, requests) = filter.push(b"x\x1b]7771;v=1;dele");
        assert_eq!(plain, b"x");
        assert!(requests.is_empty());
        let (plain, requests) = filter.push(b"te;abc\x07y");
        assert_eq!(plain, b"y");
        assert!(matches!(requests.as_slice(), [GalleryRequest::Delete { id }] if id == "abc"));
    }

    #[test]
    fn paste_pieces_become_one_png_and_nothing_else() {
        let png = b"\x89PNG fake but small";
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let (first, second) = encoded.split_at(5);
        let mut filter = GalleryFilter::new();
        let frame = format!("\x1b]7771;v=1;drop;nonce1;0;2;{first}\x07");
        let (plain, requests) = filter.push(frame.as_bytes());
        assert!(
            plain.is_empty(),
            "leaked {:?}",
            String::from_utf8_lossy(&plain)
        );
        assert!(requests.is_empty(), "completed early: {requests:?}");
        let frame = format!("\x1b]7771;v=1;drop;nonce1;1;2;{second}\x07");
        let (plain, requests) = filter.push(frame.as_bytes());
        match requests.as_slice() {
            [GalleryRequest::Paste { nonce, png: data }] => {
                assert_eq!(nonce, "nonce1");
                assert_eq!(data, png);
            }
            other => panic!(
                "expected the reassembled paste, got {other:?}, plain {:?}",
                String::from_utf8_lossy(&plain)
            ),
        }
    }

    #[test]
    fn a_frame_that_never_ends_does_not_swallow_the_input_forever() {
        let mut filter = GalleryFilter::new();
        let chunk = vec![b'a'; MAX_FRAME];
        let (plain, _) = filter.push(&chunk);
        assert!(plain.len() > MAX_FRAME - 100);
    }
}
