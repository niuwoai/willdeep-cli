//! The MuseTerm image column, spoken over the private OSC 7771 sequence.
//!
//! MuseTerm draws the thumbnails. This process owns the attachments, so it announces them and
//! answers when the terminal reports a paste or a click on a delete icon. The terminal only sends
//! those bytes after it has seen `hello`, and `hello` is only written once the filtered pty is on
//! standard input, installed before anything reads it.

#[cfg(unix)]
use std::io::Read;
use std::io::{self, IsTerminal, Write};

use base64::Engine;
#[cfg(unix)]
use uuid::Uuid;
use willdeep_core::MessageAttachment;

#[cfg(unix)]
const OSC: &[u8] = b"\x1b]7771;";
const VERSION: &str = "v=1";
#[cfg(unix)]
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
/// Nothing constructs one on Windows: the input filter that produces them is unix-only.
#[cfg_attr(not(unix), allow(dead_code))]
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

/// Tells the terminal a paste became a draft attachment. Only the unix request handler calls it.
#[cfg(unix)]
pub(crate) fn acknowledge(out: &mut impl Write, nonce: &str, id: Uuid) -> io::Result<()> {
    announce(out, "ack", &format!("{nonce};{id}"))
}

/// Tells the terminal a paste was refused, so it can drop the thumbnail it showed early.
/// Only the unix request handler calls it.
#[cfg(unix)]
pub(crate) fn reject(out: &mut impl Write, nonce: &str, reason: &str) -> io::Result<()> {
    let reason: String = reason
        .chars()
        .filter(|ch| *ch >= ' ' && *ch != ';')
        .take(80)
        .collect();
    announce(out, "reject", &format!("{nonce};{reason}"))
}

/// Confirms that a draft attachment is gone. Only the unix request handler calls it.
#[cfg(unix)]
pub(crate) fn confirm_removed(out: &mut impl Write, id: &str) -> io::Result<()> {
    announce(out, "remove", id)
}

/// Sends an image this process holds, such as a picture pasted from the TUI's own clipboard, so
/// the column can draw a thumbnail even though it never saw the bytes itself.
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

/// Bytes of one paste that have arrived so far. Only the input filter reassembles pastes, and
/// that filter is unix-only.
#[cfg(unix)]
struct PartialPaste {
    nonce: String,
    total: usize,
    pieces: Vec<Option<String>>,
}

/// Pulls gallery frames out of the terminal's input and leaves every other byte untouched.
#[cfg(unix)]
pub(crate) struct GalleryFilter {
    pending: Vec<u8>,
    pastes: Vec<PartialPaste>,
    /// When the bytes now held back first arrived. A suffix is held only while it could still
    /// become the marker, and a frame only while it could still finish.
    held_since: Option<std::time::Instant>,
}

#[cfg(unix)]
impl GalleryFilter {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::new(),
            pastes: Vec::new(),
            held_since: None,
        }
    }

    /// Returns the bytes a key parser should see, plus any request that became complete.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> (Vec<u8>, Vec<GalleryRequest>) {
        self.pending.extend_from_slice(bytes);
        let mut plain = Vec::new();
        let mut requests = Vec::new();
        loop {
            let Some(start) = marker_start(&self.pending) else {
                // Hold back only the tail that could still become the marker; a lone Esc is
                // released by `flush_expired`. Everything before it is a keystroke, and making
                // it wait for a frame that never comes swallows the arrow keys and Esc.
                let keep = retained_suffix(&self.pending);
                let drain = self.pending.len() - keep;
                plain.extend(self.pending.drain(..drain));
                break;
            };
            if start > 0 {
                plain.extend(self.pending.drain(..start));
            }
            let Some(end) = frame_end(&self.pending) else {
                if self.pending.len() > MAX_FRAME {
                    // Nothing this long can be a frame the terminal would send.
                    self.pending.clear();
                    self.pastes.clear();
                }
                break;
            };
            let frame: Vec<u8> = self.pending.drain(..end).collect();
            if let Some(request) = self.decode(&frame) {
                requests.push(request);
            }
        }
        self.held_since = if self.pending.is_empty() {
            None
        } else {
            Some(self.held_since.unwrap_or_else(std::time::Instant::now))
        };
        (plain, requests)
    }

    /// True while bytes are held back waiting for the rest of a frame.
    pub(crate) fn holding(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Releases what has been held too long: a bare marker prefix becomes a key again, and an
    /// unterminated frame is abandoned. The forwarder calls this when its poll times out.
    pub(crate) fn flush_expired(&mut self, now: std::time::Instant) -> Vec<u8> {
        let Some(since) = self.held_since else {
            return Vec::new();
        };
        let age = now.saturating_duration_since(since);
        if self.pending.starts_with(OSC) {
            if age >= INCOMPLETE_FRAME_TTL {
                self.pending.clear();
                self.pastes.clear();
                self.held_since = None;
            }
            return Vec::new();
        }
        if age >= PARTIAL_MARKER_TTL {
            self.held_since = None;
            return std::mem::take(&mut self.pending);
        }
        Vec::new()
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

#[cfg(unix)]
fn marker_start(bytes: &[u8]) -> Option<usize> {
    bytes.windows(OSC.len()).position(|window| window == OSC)
}

/// The longest tail of `bytes` that is a proper prefix of the marker. Only that tail can begin a
/// frame, so only that tail may wait for more bytes.
#[cfg(unix)]
fn retained_suffix(bytes: &[u8]) -> usize {
    let limit = bytes.len().min(OSC.len() - 1);
    (1..=limit)
        .rev()
        .find(|length| bytes.ends_with(&OSC[..*length]))
        .unwrap_or(0)
}

#[cfg(unix)]
fn frame_end(bytes: &[u8]) -> Option<usize> {
    // Only BEL. The string terminator's second byte also occurs inside base64.
    bytes
        .iter()
        .position(|byte| *byte == 0x07)
        .map(|index| index + 1)
}

/// Reads the real terminal and writes everything that is not a gallery frame into a pty that
/// the program treats as its stdin.
///
/// The filtered end must stay a terminal. crossterm asks `isatty` the first time it reads and
/// registers that descriptor with its event source; swapping in a pipe afterwards leaves the
/// source watching the real terminal while the reads go to the pipe, so keystrokes vanish. A
/// pty keeps both sides terminals, and it is installed before anything reads stdin.
///
/// Unix only. On Windows `install` returns an error, the caller never writes `hello`, and the
/// terminal never sends gallery bytes.
#[cfg(unix)]
pub(crate) struct GalleryInput;

#[cfg(unix)]
impl GalleryInput {
    pub(crate) fn install(
        tx: tokio::sync::mpsc::UnboundedSender<crate::tui::UiMessage>,
    ) -> io::Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd};

        if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "stdin is not a terminal",
            ));
        }

        let (master, slave) = open_pty()?;
        // The filtered side must deliver bytes as soon as they are written, without line
        // buffering or a second echo.
        let mut filtered_mode = terminal_attributes(slave.as_raw_fd())?;
        unsafe { libc::cfmakeraw(&mut filtered_mode) };
        set_terminal_attributes(slave.as_raw_fd(), &filtered_mode)?;

        let input = duplicate(libc::STDIN_FILENO)?;
        let restore = duplicate(libc::STDIN_FILENO)?;
        let original = terminal_attributes(restore)?;
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        set_terminal_attributes(restore, &raw)?;

        // The pty slave becomes stdin, and the saved descriptors keep the real terminal.
        if unsafe { libc::dup2(slave.as_raw_fd(), libc::STDIN_FILENO) } < 0 {
            let error = io::Error::last_os_error();
            let _ = set_terminal_attributes(restore, &original);
            unsafe { libc::close(restore) };
            return Err(error);
        }
        copy_window_size(restore, libc::STDIN_FILENO);
        drop(slave);

        let input = unsafe { std::fs::File::from_raw_fd(input) };
        if let Err(error) = std::thread::Builder::new()
            .name("gallery-input".into())
            .spawn(move || forward(input, master, tx))
        {
            // Put the real terminal back on stdin; nothing else has changed yet.
            if unsafe { libc::dup2(restore, libc::STDIN_FILENO) } < 0 {
                let restore_error = io::Error::last_os_error();
                unsafe { libc::close(restore) };
                return Err(restore_error);
            }
            let _ = set_terminal_attributes(restore, &original);
            unsafe { libc::close(restore) };
            return Err(error);
        }

        if let Ok(mut saved) = SAVED_TERMINAL.lock() {
            *saved = Some((restore, original));
        } else {
            unsafe { libc::close(restore) };
        }
        if let Ok(mut window) = WINDOW_SIZES.lock() {
            *window = Some((restore, libc::STDIN_FILENO));
        }
        ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(Self)
    }
}

#[cfg(unix)]
impl Drop for GalleryInput {
    fn drop(&mut self) {
        restore_terminal();
    }
}

#[cfg(unix)]
fn forward(
    mut input: std::fs::File,
    mut output: std::fs::File,
    tx: tokio::sync::mpsc::UnboundedSender<crate::tui::UiMessage>,
) {
    use std::os::fd::AsRawFd;

    let mut filter = GalleryFilter::new();
    let mut buffer = [0; 4096];
    let mut poll_fd = libc::pollfd {
        fd: input.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        poll_fd.revents = 0;
        // A held marker prefix (a bare Esc, say) must not wait forever for bytes that are never
        // coming. With nothing held there is no deadline, so an idle terminal costs no wakeups.
        let timeout = if filter.holding() {
            GALLERY_POLL_MILLIS
        } else {
            -1
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready > 0 {
            match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let (plain, requests) = filter.push(&buffer[..count]);
                    for request in requests {
                        if tx.send(crate::tui::UiMessage::Gallery(request)).is_err() {
                            return;
                        }
                    }
                    if !plain.is_empty() && output.write_all(&plain).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let plain = filter.flush_expired(std::time::Instant::now());
        if !plain.is_empty() && output.write_all(&plain).is_err() {
            break;
        }
    }
}

#[cfg(unix)]
fn open_pty() -> io::Result<(std::fs::File, std::fs::File)> {
    use std::os::fd::{AsRawFd, FromRawFd};

    const FLAGS: libc::c_int = libc::O_RDWR | libc::O_NOCTTY;
    let master = unsafe { libc::posix_openpt(FLAGS) };
    if master < 0 {
        return Err(io::Error::last_os_error());
    }
    let master = unsafe { std::fs::File::from_raw_fd(master) };
    if unsafe { libc::grantpt(master.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::unlockpt(master.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let name = unsafe { libc::ptsname(master.as_raw_fd()) };
    if name.is_null() {
        return Err(io::Error::last_os_error());
    }
    let slave = unsafe { libc::open(name, FLAGS) };
    if slave < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((master, unsafe { std::fs::File::from_raw_fd(slave) }))
}

#[cfg(unix)]
fn terminal_attributes(fd: std::os::fd::RawFd) -> io::Result<libc::termios> {
    let mut mode: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut mode) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

#[cfg(unix)]
fn set_terminal_attributes(fd: std::os::fd::RawFd, mode: &libc::termios) -> io::Result<()> {
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, mode) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn copy_window_size(from: std::os::fd::RawFd, to: std::os::fd::RawFd) {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(from, libc::TIOCGWINSZ, &mut size) } == 0 {
        unsafe { libc::ioctl(to, libc::TIOCSWINSZ, &size) };
    }
}

#[cfg(unix)]
fn duplicate(fd: std::os::fd::RawFd) -> io::Result<std::os::fd::RawFd> {
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(copy)
    }
}

/// How long a marker prefix waits for the rest of the sequence before it is treated as a key.
/// The terminal writes a frame in one go, so a real frame never takes this long to arrive.
#[cfg(unix)]
const PARTIAL_MARKER_TTL: std::time::Duration = std::time::Duration::from_millis(50);
/// An unterminated frame is abandoned rather than held forever.
#[cfg(unix)]
const INCOMPLETE_FRAME_TTL: std::time::Duration = std::time::Duration::from_secs(5);
/// How often the forwarder wakes to expire what it is holding.
#[cfg(unix)]
const GALLERY_POLL_MILLIS: libc::c_int = 50;

/// Set once the filtered pty sits on stdin. The event loop only writes `hello` when this is
/// true, so a terminal never sends frames to a process that cannot filter them.
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The real terminal's saved mode, plus a descriptor to put it back with.
#[cfg(unix)]
static SAVED_TERMINAL: std::sync::Mutex<Option<(std::os::fd::RawFd, libc::termios)>> =
    std::sync::Mutex::new(None);

/// The real and filtered descriptors whose window sizes are kept in step.
#[cfg(unix)]
static WINDOW_SIZES: std::sync::Mutex<Option<(std::os::fd::RawFd, std::os::fd::RawFd)>> =
    std::sync::Mutex::new(None);

/// True when the terminal on the other end speaks the gallery protocol. Only MuseTerm sets either
/// variable for a runtime session; every other terminal keeps an untouched stdin.
pub(crate) fn hosted() -> bool {
    ["MUSETERM_AGENT_CONTEXT", "QTERM_AGENT_CONTEXT"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// Installs the filter on standard input when this really is a hosted interactive TUI. The filter
/// must be in place before anything reads stdin — the session picker is one such reader — and a
/// terminal without the markers keeps an untouched stdin and never sees a `hello`.
pub(crate) fn install_hosted(
    interactive: bool,
    tx: tokio::sync::mpsc::UnboundedSender<crate::tui::UiMessage>,
) -> Option<GalleryInput> {
    if !interactive || !std::io::stdout().is_terminal() || !hosted() {
        return None;
    }
    GalleryInput::install(tx).ok()
}

/// True when the filtered pty is installed and `hello` may be written.
pub(crate) fn active() -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Puts the real terminal back the way it was. Idempotent; also called when the TUI ends, so
/// the closing lines are printed with the terminal's normal output processing.
pub(crate) fn restore_terminal() {
    ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    #[cfg(unix)]
    {
        if let Ok(mut saved) = SAVED_TERMINAL.lock()
            && let Some((fd, mode)) = saved.take()
        {
            unsafe {
                libc::tcsetattr(fd, libc::TCSANOW, &mode);
                libc::close(fd);
            }
        }
        if let Ok(mut window) = WINDOW_SIZES.lock() {
            *window = None;
        }
    }
}

/// Copies the real terminal's window size to the filtered pty, so a full-screen child such as
/// the external editor sees the size of the terminal the user is looking at.
#[cfg(unix)]
pub(crate) fn sync_window_size() {
    let Ok(window) = WINDOW_SIZES.lock() else {
        return;
    };
    if let Some((real, filtered)) = *window {
        copy_window_size(real, filtered);
    }
}

#[cfg(not(unix))]
pub(crate) fn sync_window_size() {}

/// Windows has no image column. Reporting an error keeps the caller from writing `hello`, so the
/// terminal never sends gallery bytes to a build that cannot filter them.
#[cfg(not(unix))]
#[allow(dead_code)]
pub(crate) struct GalleryInput;

#[cfg(not(unix))]
#[allow(dead_code)]
impl GalleryInput {
    pub(crate) fn install(
        _tx: tokio::sync::mpsc::UnboundedSender<crate::tui::UiMessage>,
    ) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the terminal image column needs a unix pty",
        ))
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
/// word, and a file that is not a PNG is refused. Only the unix request handler calls it.
#[cfg(unix)]
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

#[cfg(all(test, unix))]
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
    fn a_blob_is_one_frame_per_piece() {
        let mut out = Vec::new();
        announce_blob(&mut out, "11111111-2222-3333-4444-555555555555", b"png").unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b]7771;v=1;blob;11111111-2222-3333-4444-555555555555;0;1;cG5n\x07"
        );
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
    fn an_arrow_key_never_waits_for_a_frame() {
        let mut filter = GalleryFilter::new();
        let (plain, requests) = filter.push(b"\x1b[A");
        assert_eq!(plain, b"\x1b[A");
        assert!(requests.is_empty());
    }

    #[test]
    fn a_bare_escape_becomes_a_key_once_no_frame_follows() {
        let mut filter = GalleryFilter::new();
        let (plain, _) = filter.push(b"\x1b");
        assert!(plain.is_empty(), "an Esc might still begin a frame");
        let now = std::time::Instant::now();
        assert!(filter.flush_expired(now).is_empty());
        assert_eq!(filter.flush_expired(now + PARTIAL_MARKER_TTL), b"\x1b");
    }

    #[test]
    fn a_marker_prefix_split_across_reads_is_held_and_completed() {
        let mut filter = GalleryFilter::new();
        let (plain, _) = filter.push(b"ab\x1b]77");
        assert_eq!(plain, b"ab");
        let (plain, requests) = filter.push(b"71;v=1;delete;abc\x07c");
        assert_eq!(plain, b"c");
        assert!(matches!(requests.as_slice(), [GalleryRequest::Delete { id }] if id == "abc"));
    }

    #[test]
    fn an_unfinished_frame_is_abandoned() {
        let mut filter = GalleryFilter::new();
        let (plain, requests) = filter.push(b"\x1b]7771;v=1;drop;nonce1;0;2;AAAA");
        assert!(plain.is_empty());
        assert!(requests.is_empty());
        let now = std::time::Instant::now();
        assert!(filter.flush_expired(now + PARTIAL_MARKER_TTL).is_empty());
        assert!(filter.flush_expired(now + INCOMPLETE_FRAME_TTL).is_empty());
        let (plain, _) = filter.push(b"ok");
        assert_eq!(plain, b"ok");
    }

    #[test]
    fn an_oversized_frame_is_dropped_instead_of_growing_forever() {
        let mut filter = GalleryFilter::new();
        let mut chunk = b"\x1b]7771;v=1;drop;nonce1;0;2;".to_vec();
        chunk.resize(MAX_FRAME + 1, b'a');
        let (plain, _) = filter.push(&chunk);
        assert!(plain.is_empty());
        let (plain, _) = filter.push(b"ok");
        assert_eq!(plain, b"ok");
    }
}
