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
#[cfg(unix)]
const SGR_MOUSE: &[u8] = b"\x1b[<";
#[cfg(unix)]
const PASTE_START: &[u8] = b"\x1b[200~";
#[cfg(unix)]
const PASTE_END: &[u8] = b"\x1b[201~";
#[cfg(unix)]
const INPUT_MARKERS: &[&[u8]] = &[OSC, SGR_MOUSE, PASTE_START];
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

/// Ends the conversation when the TUI exits, so the column folds away with the process instead
/// of keeping the last draft's thumbnails. The empty body still writes its trailing `;`: the
/// terminal's parser wants the version field, the operation and at least one field after it.
pub(crate) fn announce_bye(out: &mut impl Write) -> io::Result<()> {
    announce(out, "bye", "")
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
    in_bracketed_paste: bool,
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
            in_bracketed_paste: false,
            held_since: None,
        }
    }

    /// Returns the bytes a key parser should see, plus any request that became complete.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> (Vec<u8>, Vec<GalleryRequest>) {
        self.pending.extend_from_slice(bytes);
        let mut plain = Vec::new();
        let mut requests = Vec::new();
        loop {
            if self.in_bracketed_paste {
                if let Some(start) = self
                    .pending
                    .windows(PASTE_END.len())
                    .position(|bytes| bytes == PASTE_END)
                {
                    plain.extend(self.pending.drain(..start + PASTE_END.len()));
                    self.in_bracketed_paste = false;
                    continue;
                }
                let drain = self.pending.len() - retained_suffix(&self.pending, &[PASTE_END]);
                plain.extend(self.pending.drain(..drain));
                break;
            }
            let Some(start) = marker_start(&self.pending) else {
                // Preserve trailing prefixes across reads, including Esc shared by both protocols.
                let keep = retained_suffix(&self.pending, INPUT_MARKERS);
                let drain = self.pending.len() - keep;
                plain.extend(self.pending.drain(..drain));
                break;
            };
            plain.extend(self.pending.drain(..start));
            if self.pending.starts_with(PASTE_START) {
                plain.extend(self.pending.drain(..PASTE_START.len()));
                self.in_bracketed_paste = true;
                continue;
            }
            if self.pending.starts_with(SGR_MOUSE) {
                // crossterm treats a lone Esc as a key, and an incomplete SGR report can
                // swallow subsequent typing. Forward complete reports and recover before
                // forwarding the first byte that doesn't belong to a malformed report.
                match mouse_report(&self.pending) {
                    MouseReport::Complete(end) => plain.extend(self.pending.drain(..end)),
                    MouseReport::Invalid(end) => drop(self.pending.drain(..end)),
                    MouseReport::Pending => break,
                }
                self.held_since = None;
                continue;
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

    /// Releases what has been held too long: a bare marker prefix becomes a key again, and an
    /// unterminated frame is abandoned. The forwarder calls this when its poll times out.
    pub(crate) fn flush_expired(&mut self, now: std::time::Instant) -> Vec<u8> {
        let Some(since) = self.held_since else {
            return Vec::new();
        };
        let age = now.saturating_duration_since(since);
        // A split paste end must stay buffered until the rest of the delimiter arrives.
        if self.in_bracketed_paste {
            return Vec::new();
        }
        if self.pending.starts_with(SGR_MOUSE) {
            if age >= INCOMPLETE_FRAME_TTL {
                self.pending.clear();
                self.held_since = None;
            }
            return Vec::new();
        }
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
            let pending = std::mem::take(&mut self.pending);
            // A standalone Esc is a real key, but an expired CSI prefix is not.
            if pending == b"\x1b[" || pending.starts_with(SGR_MOUSE) {
                return Vec::new();
            }
            return pending;
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
    bytes.iter().enumerate().find_map(|(index, byte)| {
        (*byte == b'\x1b'
            && INPUT_MARKERS
                .iter()
                .any(|prefix| bytes[index..].starts_with(prefix)))
        .then_some(index)
    })
}

/// The longest tail that could still start a frame requiring inspection.
#[cfg(unix)]
fn retained_suffix(bytes: &[u8], markers: &[&[u8]]) -> usize {
    markers
        .iter()
        .map(|prefix| {
            (1..prefix.len())
                .rev()
                .find(|length| bytes.ends_with(&prefix[..*length]))
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

#[cfg(unix)]
enum MouseReport {
    Complete(usize),
    Invalid(usize),
    Pending,
}

#[cfg(unix)]
fn mouse_report(bytes: &[u8]) -> MouseReport {
    // SGR encodes an 8-bit button field and two 1-based 16-bit coordinates.
    let fields = [
        (0, u32::from(u8::MAX)),
        (1, u32::from(u16::MAX)),
        (1, u32::from(u16::MAX)),
    ];
    let mut index = SGR_MOUSE.len();
    let mut valid = true;
    for (field, (min, max)) in fields.into_iter().enumerate() {
        let start = index;
        let mut value = 0;
        while let Some(digit @ b'0'..=b'9') = bytes.get(index).copied() {
            index += 1;
            value = value * 10 + u32::from(digit - b'0');
            // Bound waiting even when there are leading zeros.
            if value > max || index - start > max.ilog10() as usize + 1 {
                value = max + 1;
            }
        }
        let last = field == fields.len() - 1;
        valid &= index > start && (min..=max).contains(&value);
        if !last && bytes.get(index) == Some(&b';') {
            index += 1;
            continue;
        }
        if last && bytes.get(index) == Some(&b';') {
            index += 1;
        }
        return match bytes.get(index) {
            None => MouseReport::Pending,
            Some(b'M' | b'm') if last && valid => MouseReport::Complete(index + 1),
            Some(b'M' | b'm') => MouseReport::Invalid(index + 1),
            Some(_) => MouseReport::Invalid(index),
        };
    }
    unreachable!("the final mouse field consumes the terminator")
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
        let master_fd = master.as_raw_fd();
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
        // 退出时靠它把 pty 里还没被读走的按键排空。不排的话，进程退出拆掉 pty 时，
        // 这些字节会连同 Ctrl+C 一起退回真实终端，那时鼠标追踪已经关了，全变成乱码。
        if let Ok(mut drain) = INPUT_DRAIN.lock() {
            *drain = Some(master_fd);
            INPUT_DRAIN_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
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
    // 退出时要读完 pty 里剩下的字节，不能卡在一个永远等不到数据的 poll 上。
    let master_fd = output.as_raw_fd();
    'forward: loop {
        poll_fd.revents = 0;
        // 固定节拍：扣着的半截标记要靠它过期；退出排空也只有靠它才能把阻塞在 poll 上的
        // 线程叫醒。空闲时每 50ms 醒一次，换的是按键不压在过滤器里、退出不等满 200ms。
        let ready = unsafe { libc::poll(&mut poll_fd, 1, GALLERY_POLL_MILLIS) };
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
                            break 'forward;
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
        // 退出已开始：把 pty 里还没读走的字节读完，让它们被正常处理，而不是在进程
        // 退出拆掉 pty 时退回真实终端。读完就清掉请求，退出路径才知道可以继续。
        if input_drain_requested(master_fd) && drain_pty_input(&mut input, &mut output, &mut filter)
        {
            clear_input_drain(master_fd);
            break;
        }
    }
    // 线程不在了，这个 fd 也不再有人排空。留给退出路径只会让它空等 200ms。
    release_input_drain(master_fd);
}

/// 退出时读完 pty 里剩下的输入。读到空或读不到为止，返回 true 表示可以收工。
#[cfg(unix)]
fn drain_pty_input(
    input: &mut std::fs::File,
    output: &mut std::fs::File,
    filter: &mut GalleryFilter,
) -> bool {
    use std::os::fd::AsRawFd;

    let mut buffer = [0; 4096];
    let mut poll_fd = libc::pollfd {
        fd: input.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        poll_fd.revents = 0;
        let ready = unsafe { libc::poll(&mut poll_fd, 1, 0) };
        if ready <= 0 {
            return true;
        }
        match input.read(&mut buffer) {
            Ok(0) => return true,
            Ok(count) => {
                let (plain, _) = filter.push(&buffer[..count]);
                if !plain.is_empty() && output.write_all(&plain).is_err() {
                    return true;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
}

/// 退出路径请求转发线程把这个 pty 里剩下的输入读完。
/// 只有请求位真的置上才算数：安装时记下的 fd 只是「该等谁」。
#[cfg(unix)]
fn input_drain_requested(master_fd: std::os::fd::RawFd) -> bool {
    INPUT_DRAIN_REQUESTED.load(std::sync::atomic::Ordering::SeqCst)
        && INPUT_DRAIN
            .lock()
            .is_ok_and(|drain| *drain == Some(master_fd))
}

/// 转发线程读完后清掉请求。只清自己的 fd，避免误清别人的。
#[cfg(unix)]
fn clear_input_drain(master_fd: std::os::fd::RawFd) {
    if INPUT_DRAIN
        .lock()
        .is_ok_and(|drain| *drain == Some(master_fd))
    {
        INPUT_DRAIN_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 转发线程退出后把记下的 fd 一并收掉：没有线程了，再请求也只会空等。
#[cfg(unix)]
fn release_input_drain(master_fd: std::os::fd::RawFd) {
    if let Ok(mut drain) = INPUT_DRAIN.lock()
        && *drain == Some(master_fd)
    {
        *drain = None;
        INPUT_DRAIN_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 让转发线程开始排空，并等它读完。读不完也不再等下去：后面还有 tcflush 兜底。
#[cfg(unix)]
fn request_input_drain() {
    let Some(master_fd) = INPUT_DRAIN.lock().ok().and_then(|drain| *drain) else {
        return;
    };
    INPUT_DRAIN_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        let drained = !INPUT_DRAIN_REQUESTED.load(std::sync::atomic::Ordering::SeqCst)
            || INPUT_DRAIN
                .lock()
                .is_ok_and(|drain| *drain != Some(master_fd));
        if drained {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
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

/// 退出排空要找的 pty 主端。安装时记下，只为退出路径知道该等谁；它本身不是请求。
#[cfg(unix)]
static INPUT_DRAIN: std::sync::Mutex<Option<std::os::fd::RawFd>> = std::sync::Mutex::new(None);

/// 退出路径真正发起的排空请求。与上一条分开：安装时记 fd、退出时发请求，
/// 若共用一个字段，转发线程会把「安装」当成「马上退出」，启动几十毫秒后自己死掉。
#[cfg(unix)]
static INPUT_DRAIN_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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

/// 把文本写进终端自己的剪贴板（OSC 52）。MuseTerm 在本机实现这条序列，
/// 所以 SSH 远端的 willdeep 也能写对用户 Mac 的剪贴板；不支持的终端由调用方
/// 回落到 arboard，不会因为这里返回错误就宣称复制成功。
/// tmux 里必须套 DCS passthrough，并把它内部的 ESC 加倍，否则序列会被 tmux 吞掉。
pub(crate) fn write_osc52(text: &str) -> io::Result<()> {
    if !io::stdout().is_terminal() {
        return Err(io::Error::other("stdout is not a terminal"));
    }
    // GNU screen 没有可靠的 passthrough，交给调用方回落，别发一串被吞掉的字节。
    if std::env::var_os("STY").is_some() && std::env::var_os("TMUX").is_none() {
        return Err(io::Error::other("OSC 52 is not supported under GNU screen"));
    }
    let mut out = io::stdout().lock();
    write_osc52_to(&mut out, text, std::env::var_os("TMUX").is_some())?;
    out.flush()
}

fn write_osc52_to(out: &mut impl Write, text: &str, tmux: bool) -> io::Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    if tmux {
        // ESC P tmux ; <inner, every ESC doubled> ESC \
        write!(out, "\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\")
    } else {
        // BEL 结束：base64 里可能出现 0x5c，ESC ST 会被它提前截断。
        write!(out, "\x1b]52;c;{encoded}\x07")
    }
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
        // 先让转发线程把 pty 里积压的按键读完。读不完就退的话，进程退出拆掉 pty
        // 时这些字节会退回真实终端，鼠标追踪和 raw 模式那时都已经关了，用户看到的
        // 就是一串 `^[[<…M` 和 `^C`。
        request_input_drain();
        if let Ok(mut saved) = SAVED_TERMINAL.lock()
            && let Some((fd, mode)) = saved.take()
        {
            unsafe {
                // 回显恢复之前丢掉真实终端输入队列里的残留，否则同样会原样显示出来。
                libc::tcflush(fd, libc::TCIFLUSH);
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
    use std::os::fd::AsRawFd;

    #[test]
    fn plain_text_passes_through_untouched() {
        let mut filter = GalleryFilter::new();
        let (plain, requests) = filter.push(b"hello \x1b[A there");
        assert_eq!(plain, b"hello \x1b[A there");
        assert!(requests.is_empty());
    }

    /// 退出排空用的主端 fd 在安装时就要记下来，但「记下 fd」不等于「现在开始排空」。
    /// rc2 把两件事挤在 `INPUT_DRAIN: Option<RawFd>` 里，转发线程第一圈看到 fd 就当成
    /// 退出请求，启动约 50ms 后自己 break——之后所有按键都停在真实终端里，Ctrl+C
    /// 再也到不了 TUI（#167）。这个测试盯着线程必须活着。
    #[test]
    fn an_armed_drain_descriptor_does_not_stop_the_forwarder() {
        let _serial = DRAIN_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (master, slave) = open_pty().unwrap();
        let master_fd = master.as_raw_fd();
        // 安装时就是这个顺序：fd 已记下，但没有任何人请求排空。
        *INPUT_DRAIN.lock().unwrap() = Some(master_fd);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("gallery-armed-test".into())
            .spawn(move || {
                forward(slave, master, tx);
                let _ = done_tx.send(());
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let exited = done_rx.try_recv().is_ok();
        *INPUT_DRAIN.lock().unwrap() = None;
        assert!(
            !exited,
            "forwarder exited while the exit drain was only armed, not requested"
        );
        // 线程还阻塞在 poll 上，测试进程结束时会一起收掉；不 join，免得把它等死。
        drop(worker);
    }

    /// 两个测试都改全局的排空状态，串行执行，互不看见对方的中途值。
    static DRAIN_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn queued_input_is_drained_before_the_pty_is_torn_down() {
        let _serial = DRAIN_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // 复现 #167：事件循环忙着别的事时按键堆在 pty 里，进程一退就原样退回真实终端。
        // 退出前必须把它们读完。
        let (master, slave) = open_pty().unwrap();
        let master_fd = master.as_raw_fd();
        // 稍后从主端写一个字节把转发线程从 poll 上叫醒；主端本身被移进线程。
        let wake = duplicate(master_fd).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        // 转发线程读真实终端（这里用 slave 充当）、写进 pty 主端。
        let worker = std::thread::Builder::new()
            .name("gallery-drain-test".into())
            .spawn(move || {
                forward(slave, master, tx);
                let _ = done_tx.send(());
            })
            .unwrap();
        if let Ok(mut drain) = INPUT_DRAIN.lock() {
            *drain = Some(master_fd);
        }
        INPUT_DRAIN_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
        // 不完整的帧标记：过滤器会把它扣住，不会回写，只用来叫醒阻塞在 poll 上的线程。
        let written = unsafe { libc::write(wake, b"\x1b]7771;".as_ptr().cast(), 7) };
        assert_eq!(written, 7);
        unsafe { libc::close(wake) };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let drained = INPUT_DRAIN
                .lock()
                .is_ok_and(|drain| drain.is_none_or(|fd| fd != master_fd));
            if drained {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            INPUT_DRAIN
                .lock()
                .is_ok_and(|drain| drain.is_none_or(|fd| fd != master_fd)),
            "forwarder did not finish draining (thread exited: {})",
            done_rx.try_recv().is_ok()
        );
        let _ = worker.join();
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

    /// 退出帧必须带末尾分号：终端的解析器要求版本、操作之外至少还有一个字段。
    #[test]
    fn a_bye_frame_keeps_the_trailing_semicolon() {
        let mut out = Vec::new();
        announce_bye(&mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]7771;v=1;bye;\x07");
    }

    /// OSC 52 用 BEL 结束、正文是 base64：MuseTerm 据此把远端代码写进本机剪贴板。
    #[test]
    fn an_osc52_frame_carries_base64_clipboard_text() {
        let mut out = Vec::new();
        write_osc52_to(&mut out, "hi", false).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]52;c;aGk=\x07");
    }

    /// tmux 只认 DCS passthrough，且内层 ESC 必须写两遍，否则序列到不了外层终端。
    #[test]
    fn an_osc52_frame_inside_tmux_is_dcs_wrapped() {
        let mut out = Vec::new();
        write_osc52_to(&mut out, "hi", true).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\"
        );
    }

    /// 空列表是「收起这一列」，新进程启动后靠它清掉上一轮残留的缩略图。
    #[cfg(unix)]
    #[test]
    fn fragmented_mouse_report_reaches_the_key_parser_as_one_packet() {
        let report = b"\x1b[<35;49;20M";
        for split in 1..report.len() {
            let mut filter = GalleryFilter::new();
            let (plain, requests) = filter.push(&report[..split]);
            assert!(plain.is_empty(), "split {split}");
            assert!(requests.is_empty());
            let (plain, requests) = filter.push(&report[split..]);
            assert_eq!(plain, report, "split {split}");
            assert!(requests.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn incomplete_mouse_reports_do_not_swallow_following_typing() {
        let mut filter = GalleryFilter::new();
        assert!(filter.push(b"\x1b[<35;49;").0.is_empty());
        assert_eq!(
            filter.push("打字\x1b[A".as_bytes()).0,
            "打字\x1b[A".as_bytes()
        );

        assert!(filter.push(b"\x1b[<35;49;20").0.is_empty());
        let expired = std::time::Instant::now() + PARTIAL_MARKER_TTL;
        assert!(filter.flush_expired(expired).is_empty());
        assert_eq!(filter.push(b"continue\r").0, b"continue\r");
    }

    #[cfg(unix)]
    #[test]
    fn invalid_mouse_coordinates_are_discarded_before_key_decoding() {
        let mut filter = GalleryFilter::new();
        assert_eq!(filter.push(b"\x1b[<35;49;0Mtext").0, b"text");
        assert_eq!(filter.push(b"\x1b[<35;49;65536Mtext").0, b"text");
        assert_eq!(filter.push(b"\x1b[<35;0;20Mtext").0, b"text");
        assert_eq!(filter.push(b"\x1b[<0;20;10;M").0, b"\x1b[<0;20;10;M");
        assert_eq!(filter.push(b"[<35;49;20M").0, b"[<35;49;20M");
    }

    #[cfg(unix)]
    #[test]
    fn bracketed_paste_keeps_mouse_like_text_and_split_markers() {
        let paste = b"\x1b[200~hello\x1b[<35;0;20M\x1b[201~";
        for split in 1..paste.len() {
            let mut filter = GalleryFilter::new();
            let (mut plain, before) = filter.push(&paste[..split]);
            let (after, requests) = filter.push(&paste[split..]);
            plain.extend(after);
            assert_eq!(plain, paste, "split {split}");
            assert!(before.is_empty() && requests.is_empty());
            assert_eq!(filter.push(b"\x1b[<35;0;20Mtext").0, b"text");
        }
    }

    #[cfg(unix)]
    #[test]
    fn mouse_tail_can_arrive_after_the_short_escape_window() {
        let mut filter = GalleryFilter::new();
        assert!(filter.push(b"\x1b[<35;49;").0.is_empty());
        assert!(
            filter
                .flush_expired(std::time::Instant::now() + PARTIAL_MARKER_TTL)
                .is_empty()
        );
        assert_eq!(filter.push(b"20Mtext").0, b"\x1b[<35;49;20Mtext");
    }

    #[test]
    fn an_empty_list_folds_the_column_away() {
        let mut out = Vec::new();
        announce_list(&mut out, &[]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]7771;v=1;list;[]\x07");
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
