//! A small X11 protocol client: the connection set-up (with
//! MIT-MAGIC-COOKIE-1 authorisation from the Xauthority file), the core
//! requests the app needs, RandR monitor geometry, and event decoding. It
//! talks to the X server (or XWayland) over its socket; no Xlib.
//!
//! Requests are buffered and flushed before waiting for a reply or for the
//! next event. Replies and errors are matched to their requests by
//! sequence number; events arriving meanwhile are queued in order.

use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

pub type Window = u32;
pub type Atom = u32;

pub const EV_KEY_PRESS: u32 = 1;
pub const EV_BUTTON_PRESS: u32 = 4;
pub const EV_BUTTON_RELEASE: u32 = 8;
pub const EV_ENTER: u32 = 0x10;
pub const EV_LEAVE: u32 = 0x20;
pub const EV_POINTER_MOTION: u32 = 0x40;
pub const EV_EXPOSURE: u32 = 0x8000;
pub const EV_STRUCTURE: u32 = 0x2_0000;
pub const EV_FOCUS: u32 = 0x20_0000;
pub const EV_PROPERTY: u32 = 0x40_0000;

pub const CW_BACK_PIXEL: u32 = 0x2;
pub const CW_BORDER_PIXEL: u32 = 0x8;
pub const CW_OVERRIDE_REDIRECT: u32 = 0x200;
pub const CW_EVENT_MASK: u32 = 0x800;
pub const CW_COLORMAP: u32 = 0x2000;
pub const CW_CURSOR: u32 = 0x4000;

pub const CFG_X: u16 = 1;
pub const CFG_Y: u16 = 2;
pub const CFG_W: u16 = 4;
pub const CFG_H: u16 = 8;
pub const CFG_STACK: u16 = 64;

pub const STATE_SHIFT: u16 = 1;
pub const STATE_CONTROL: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Visual {
    pub id: u32,
    pub depth: u8,
}

pub struct Setup {
    pub root: Window,
    pub root_depth: u8,
    pub width: u16,
    pub height: u16,
    pub min_keycode: u8,
    pub max_keycode: u8,
    pub max_request_bytes: usize,
    pub argb: Option<Visual>,
    id_base: u32,
    id_mask: u32,
}

#[derive(Debug, Clone)]
pub enum Event {
    Key {
        press: bool,
        code: u8,
        state: u16,
        window: Window,
        time: u32,
    },
    Button {
        press: bool,
        button: u8,
        state: u16,
        window: Window,
        x: i32,
        y: i32,
        root_x: i32,
        root_y: i32,
        time: u32,
    },
    Motion {
        window: Window,
        x: i32,
        y: i32,
        state: u16,
        time: u32,
    },
    Crossing {
        enter: bool,
        window: Window,
        mode: u8,
        time: u32,
    },
    Focus {
        gained: bool,
        window: Window,
        mode: u8,
        detail: u8,
    },
    Expose {
        window: Window,
        count: u16,
    },
    Configure {
        window: Window,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    /// Map, unmap, reparent and destroy notifications (not acted on).
    Structure,
    Client {
        window: Window,
        kind: Atom,
        data: [u32; 5],
    },
    Property {
        window: Window,
        atom: Atom,
    },
    SelectionRequest {
        time: u32,
        requestor: Window,
        selection: Atom,
        target: Atom,
        property: Atom,
    },
    SelectionNotify {
        requestor: Window,
        property: Atom,
    },
    SelectionClear {
        selection: Atom,
    },
    Mapping,
    Other,
}

enum Stream {
    Unix(UnixStream),
    Tcp(std::net::TcpStream),
}

impl Stream {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Unix(s) => s.read(b),
            Stream::Tcp(s) => s.read(b),
        }
    }
    fn write_all(&mut self, b: &[u8]) -> std::io::Result<()> {
        match self {
            Stream::Unix(s) => s.write_all(b),
            Stream::Tcp(s) => s.write_all(b),
        }
    }
    fn fd(&self) -> RawFd {
        match self {
            Stream::Unix(s) => s.as_raw_fd(),
            Stream::Tcp(s) => s.as_raw_fd(),
        }
    }
    fn set_nonblocking(&self, v: bool) -> std::io::Result<()> {
        match self {
            Stream::Unix(s) => s.set_nonblocking(v),
            Stream::Tcp(s) => s.set_nonblocking(v),
        }
    }
}

pub struct Conn {
    stream: Stream,
    pub setup: Setup,
    seq: u16,
    out: Vec<u8>,
    inbuf: Vec<u8>,
    events: VecDeque<Event>,
    replies: HashMap<u16, Option<Vec<u8>>>,
    next_id: u32,
    atoms: HashMap<String, Atom>,
    randr: Option<u8>,
    /// The Shape extension's major opcode.
    pub shape: Option<u8>,
    keysyms: Vec<u32>,
    per_code: usize,
    /// Set when the connection broke; the app then quits.
    pub dead: bool,
}

fn pad4(n: usize) -> usize {
    (4 - n % 4) % 4
}

fn get_u16(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2)
        .map_or(0, |v| u16::from_le_bytes([v[0], v[1]]))
}
fn get_i16(b: &[u8], i: usize) -> i32 {
    i32::from(get_u16(b, i) as i16)
}
fn get_u32(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4)
        .map_or(0, |v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}

/// DISPLAY -> (host, display number). ":1", "unix:0.0", "host:0", "/path:0".
pub fn parse_display(d: &str) -> Option<(String, u32)> {
    let colon = d.rfind(':')?;
    let host = &d[..colon];
    let num = d[colon + 1..].split('.').next()?.parse().ok()?;
    Some((host.to_string(), num))
}

/// MIT-MAGIC-COOKIE-1 for (host, display) from an Xauthority file.
pub fn find_cookie(data: &[u8], hostname: &str, display: u32) -> Option<Vec<u8>> {
    let mut i = 0;
    let field = |i: &mut usize| -> Option<Vec<u8>> {
        let n = usize::from(u16::from_be_bytes(data.get(*i..*i + 2)?.try_into().ok()?));
        let v = data.get(*i + 2..*i + 2 + n)?.to_vec();
        *i += 2 + n;
        Some(v)
    };
    let want = display.to_string();
    let mut wild = None;
    while i + 2 <= data.len() {
        let family = u16::from_be_bytes(data.get(i..i + 2)?.try_into().ok()?);
        i += 2;
        let addr = field(&mut i)?;
        let num = field(&mut i)?;
        let name = field(&mut i)?;
        let cookie = field(&mut i)?;
        if name != b"MIT-MAGIC-COOKIE-1" || !(num.is_empty() || num == want.as_bytes()) {
            continue;
        }
        match family {
            // FamilyLocal: our hostname.
            256 if addr == hostname.as_bytes() => return Some(cookie),
            // FamilyWild, or any local entry as a fallback.
            65535 | 256 => {
                wild.get_or_insert(cookie);
            }
            _ => {}
        }
    }
    wild
}

impl Conn {
    pub fn connect() -> Option<Conn> {
        let display = std::env::var("DISPLAY").ok()?;
        let (host, num) = parse_display(&display)?;
        let stream = if host.is_empty() || host == "unix" || host.starts_with('/') {
            let path = if host.starts_with('/') {
                host.clone()
            } else {
                format!("/tmp/.X11-unix/X{num}")
            };
            let abstract_ = {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(path.as_bytes())
                    .ok()
                    .and_then(|a| UnixStream::connect_addr(&a).ok())
            };
            Stream::Unix(abstract_.or_else(|| UnixStream::connect(&path).ok())?)
        } else {
            let port = 6000 + u16::try_from(num).ok()?;
            Stream::Tcp(std::net::TcpStream::connect((host.as_str(), port)).ok()?)
        };
        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let auth_file = std::env::var_os("XAUTHORITY")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".Xauthority"))
            });
        let cookie = auth_file
            .and_then(|f| std::fs::read(f).ok())
            .filter(|d| d.len() < 1 << 20)
            .and_then(|d| find_cookie(&d, &hostname, num));
        Conn::handshake(stream, cookie)
    }

    fn handshake(mut stream: Stream, cookie: Option<Vec<u8>>) -> Option<Conn> {
        let (name, data): (&[u8], &[u8]) = match &cookie {
            Some(c) => (b"MIT-MAGIC-COOKIE-1", c),
            None => (b"", b""),
        };
        let mut req = vec![b'l', 0, 11, 0, 0, 0];
        req.extend_from_slice(&(name.len() as u16).to_le_bytes());
        req.extend_from_slice(&(data.len() as u16).to_le_bytes());
        req.extend_from_slice(&[0, 0]);
        req.extend_from_slice(name);
        req.extend(std::iter::repeat_n(0, pad4(name.len())));
        req.extend_from_slice(data);
        req.extend(std::iter::repeat_n(0, pad4(data.len())));
        stream.write_all(&req).ok()?;
        let mut head = [0u8; 8];
        read_exact(&mut stream, &mut head)?;
        let extra = usize::from(get_u16(&head, 6)) * 4;
        let mut body = vec![0u8; extra];
        read_exact(&mut stream, &mut body)?;
        if head[0] != 1 {
            return None;
        }
        let setup = parse_setup(&body)?;
        stream.set_nonblocking(true).ok()?;
        let mut c = Conn {
            stream,
            next_id: 0,
            setup,
            seq: 0,
            out: Vec::new(),
            inbuf: Vec::new(),
            events: VecDeque::new(),
            replies: HashMap::new(),
            atoms: HashMap::new(),
            randr: None,
            shape: None,
            keysyms: Vec::new(),
            per_code: 0,
            dead: false,
        };
        c.load_keymap();
        if let Some((major, _)) = c.query_extension("RANDR") {
            // GetMonitors needs RandR 1.5.
            let s = c.request(major, 0, &[1, 0, 0, 0, 5, 0, 0, 0]);
            if let Some(r) = c.reply(s)
                && (get_u32(&r, 8), get_u32(&r, 12)) >= (1, 5)
            {
                c.randr = Some(major);
            }
        }
        c.shape = c.query_extension("SHAPE").map(|(major, _)| major);
        Some(c)
    }

    pub fn fd(&self) -> RawFd {
        self.stream.fd()
    }

    pub fn gen_id(&mut self) -> u32 {
        self.next_id += 1;
        self.setup.id_base | (self.next_id & self.setup.id_mask)
    }

    // -- wire --------------------------------------------------------------------

    /// Queue a request; returns its sequence number.
    pub fn request(&mut self, opcode: u8, data: u8, body: &[u8]) -> u16 {
        let len = 4 + body.len() + pad4(body.len());
        self.out.push(opcode);
        self.out.push(data);
        self.out
            .extend_from_slice(&((len / 4) as u16).to_le_bytes());
        self.out.extend_from_slice(body);
        self.out.extend(std::iter::repeat_n(0, pad4(body.len())));
        self.seq = self.seq.wrapping_add(1);
        if self.out.len() > 1 << 16 {
            self.flush();
        }
        self.seq
    }

    pub fn flush(&mut self) {
        if self.out.is_empty() || self.dead {
            self.out.clear();
            return;
        }
        let out = std::mem::take(&mut self.out);
        let mut at = 0;
        while at < out.len() {
            let r = match &mut self.stream {
                Stream::Unix(s) => s.write(&out[at..]),
                Stream::Tcp(s) => s.write(&out[at..]),
            };
            match r {
                Ok(0) => {
                    self.dead = true;
                    return;
                }
                Ok(n) => at += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    super::super::unix::sys::wait_writable(self.fd(), 1000);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.dead = true;
                    return;
                }
            }
        }
    }

    /// Read what the server has sent; with `block`, wait for something.
    pub fn read_input(&mut self, block: bool) {
        let mut buf = [0u8; 8192];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    self.dead = true;
                    break;
                }
                Ok(n) => {
                    self.inbuf.extend_from_slice(&buf[..n]);
                    if n < buf.len() {
                        break;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if block && self.inbuf.len() < 32 {
                        super::super::unix::sys::wait_readable(&[self.fd()], 1000);
                        continue;
                    }
                    break;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.dead = true;
                    break;
                }
            }
        }
        self.parse_input();
    }

    fn parse_input(&mut self) {
        loop {
            if self.inbuf.len() < 32 {
                return;
            }
            let kind = self.inbuf[0];
            let len = if kind == 1 || kind & 0x7F == 35 {
                32 + get_u32(&self.inbuf, 4) as usize * 4
            } else {
                32
            };
            if len > 1 << 24 {
                self.dead = true;
                return;
            }
            if self.inbuf.len() < len {
                return;
            }
            let packet: Vec<u8> = self.inbuf.drain(..len).collect();
            let seq = get_u16(&packet, 2);
            match kind {
                0 => {
                    self.replies.insert(seq, None);
                }
                1 => {
                    self.replies.insert(seq, Some(packet));
                }
                _ => self.events.push_back(decode_event(&packet)),
            }
        }
    }

    /// Wait for the reply to request `seq` (None on an error).
    pub fn reply(&mut self, seq: u16) -> Option<Vec<u8>> {
        self.flush();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(r) = self.replies.remove(&seq) {
                return r;
            }
            if self.dead {
                return None;
            }
            self.read_input(true);
        }
        None
    }

    /// Drop replies nobody waits for (errors from fire-and-forget requests).
    pub fn discard_stale(&mut self) {
        let seq = self.seq;
        self.replies.retain(|&s, _| seq.wrapping_sub(s) < 1000);
    }

    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    // -- requests ------------------------------------------------------------------

    pub fn atom(&mut self, name: &str) -> Atom {
        if let Some(&a) = self.atoms.get(name) {
            return a;
        }
        let mut body = Vec::new();
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(name.as_bytes());
        let s = self.request(16, 0, &body);
        let a = self.reply(s).map_or(0, |r| get_u32(&r, 8));
        if a != 0 {
            self.atoms.insert(name.to_string(), a);
        }
        a
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_window(
        &mut self,
        wid: Window,
        parent: Window,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        depth: u8,
        visual: u32,
        mask: u32,
        values: &[u32],
    ) {
        let mut b = Vec::with_capacity(28 + 4 * values.len());
        b.extend_from_slice(&wid.to_le_bytes());
        b.extend_from_slice(&parent.to_le_bytes());
        b.extend_from_slice(&(x as i16).to_le_bytes());
        b.extend_from_slice(&(y as i16).to_le_bytes());
        b.extend_from_slice(&(w.clamp(1, 32767) as u16).to_le_bytes());
        b.extend_from_slice(&(h.clamp(1, 32767) as u16).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // border
        b.extend_from_slice(&1u16.to_le_bytes()); // InputOutput
        b.extend_from_slice(&visual.to_le_bytes());
        b.extend_from_slice(&mask.to_le_bytes());
        for v in values {
            b.extend_from_slice(&v.to_le_bytes());
        }
        self.request(1, depth, &b);
    }

    pub fn change_attributes(&mut self, w: Window, mask: u32, values: &[u32]) {
        let mut b = Vec::new();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&mask.to_le_bytes());
        for v in values {
            b.extend_from_slice(&v.to_le_bytes());
        }
        self.request(2, 0, &b);
    }

    pub fn destroy_window(&mut self, w: Window) {
        self.request(4, 0, &w.to_le_bytes());
    }

    pub fn map_window(&mut self, w: Window) {
        self.request(8, 0, &w.to_le_bytes());
    }

    pub fn configure(&mut self, w: Window, mask: u16, values: &[i32]) {
        let mut b = Vec::new();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&mask.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        for v in values {
            b.extend_from_slice(&(*v as u32).to_le_bytes());
        }
        self.request(12, 0, &b);
    }

    pub fn change_property(&mut self, w: Window, prop: Atom, kind: Atom, format: u8, data: &[u8]) {
        let unit = usize::from(format / 8).max(1);
        let mut b = Vec::with_capacity(20 + data.len());
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&prop.to_le_bytes());
        b.extend_from_slice(&kind.to_le_bytes());
        b.push(format);
        b.extend_from_slice(&[0, 0, 0]);
        b.extend_from_slice(&((data.len() / unit) as u32).to_le_bytes());
        b.extend_from_slice(data);
        self.request(18, 0, &b);
    }

    pub fn change_property32(&mut self, w: Window, prop: Atom, kind: Atom, values: &[u32]) {
        let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.change_property(w, prop, kind, 32, &data);
    }

    /// (type, format, data) of a property, at most `max_bytes` of it.
    pub fn get_property(
        &mut self,
        w: Window,
        prop: Atom,
        delete: bool,
        max_bytes: u32,
    ) -> Option<(Atom, u8, Vec<u8>)> {
        let mut b = Vec::new();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&prop.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&max_bytes.div_ceil(4).to_le_bytes());
        let s = self.request(20, u8::from(delete), &b);
        let r = self.reply(s)?;
        let kind = get_u32(&r, 8);
        let format = r[1];
        let n = get_u32(&r, 16) as usize * usize::from(format / 8);
        let data = r.get(32..32 + n)?.to_vec();
        (kind != 0).then_some((kind, format, data))
    }

    pub fn set_selection_owner(&mut self, owner: Window, selection: Atom, time: u32) {
        let mut b = Vec::new();
        b.extend_from_slice(&owner.to_le_bytes());
        b.extend_from_slice(&selection.to_le_bytes());
        b.extend_from_slice(&time.to_le_bytes());
        self.request(22, 0, &b);
    }

    pub fn get_selection_owner(&mut self, selection: Atom) -> Window {
        let s = self.request(23, 0, &selection.to_le_bytes());
        self.reply(s).map_or(0, |r| get_u32(&r, 8))
    }

    pub fn convert_selection(
        &mut self,
        requestor: Window,
        selection: Atom,
        target: Atom,
        prop: Atom,
        time: u32,
    ) {
        let mut b = Vec::new();
        for v in [requestor, selection, target, prop, time] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        self.request(24, 0, &b);
    }

    pub fn send_event(&mut self, dest: Window, mask: u32, event: &[u8; 32]) {
        let mut b = Vec::with_capacity(40);
        b.extend_from_slice(&dest.to_le_bytes());
        b.extend_from_slice(&mask.to_le_bytes());
        b.extend_from_slice(event);
        self.request(25, 0, &b);
    }

    /// A ClientMessage event with five 32-bit values.
    pub fn client_message(window: Window, kind: Atom, data: [u32; 5]) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[0] = 33;
        e[1] = 32;
        e[4..8].copy_from_slice(&window.to_le_bytes());
        e[8..12].copy_from_slice(&kind.to_le_bytes());
        for (i, v) in data.iter().enumerate() {
            e[12 + 4 * i..16 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        e
    }

    /// Grab the pointer for a pop-up (owner events); true on success.
    pub fn grab_pointer(&mut self, w: Window, time: u32) -> bool {
        let mut b = Vec::new();
        b.extend_from_slice(&w.to_le_bytes());
        let mask =
            (EV_BUTTON_PRESS | EV_BUTTON_RELEASE | EV_POINTER_MOTION | EV_ENTER | EV_LEAVE) as u16;
        b.extend_from_slice(&mask.to_le_bytes());
        b.extend_from_slice(&[1, 1]); // async pointer and keyboard
        b.extend_from_slice(&0u32.to_le_bytes()); // confine-to None
        b.extend_from_slice(&0u32.to_le_bytes()); // cursor None
        b.extend_from_slice(&time.to_le_bytes());
        let s = self.request(26, 1, &b);
        self.reply(s).is_some_and(|r| r[1] == 0)
    }

    pub fn ungrab_pointer(&mut self, time: u32) {
        self.request(27, 0, &time.to_le_bytes());
    }

    pub fn grab_keyboard(&mut self, w: Window, time: u32) -> bool {
        let mut b = Vec::new();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&time.to_le_bytes());
        b.extend_from_slice(&[1, 1, 0, 0]);
        let s = self.request(31, 1, &b);
        self.reply(s).is_some_and(|r| r[1] == 0)
    }

    pub fn ungrab_keyboard(&mut self, time: u32) {
        self.request(32, 0, &time.to_le_bytes());
    }

    /// The pointer position on the screen and the button/modifier mask.
    pub fn query_pointer(&mut self) -> (i32, i32, u16) {
        let root = self.setup.root;
        let s = self.request(38, 0, &root.to_le_bytes());
        self.reply(s).map_or((0, 0, 0), |r| {
            (get_i16(&r, 16), get_i16(&r, 18), get_u16(&r, 24))
        })
    }

    /// (x, y) in `src` -> coordinates in `dst`.
    pub fn translate(&mut self, src: Window, dst: Window, x: i32, y: i32) -> Option<(i32, i32)> {
        let mut b = Vec::new();
        b.extend_from_slice(&src.to_le_bytes());
        b.extend_from_slice(&dst.to_le_bytes());
        b.extend_from_slice(&(x as i16).to_le_bytes());
        b.extend_from_slice(&(y as i16).to_le_bytes());
        let s = self.request(40, 0, &b);
        self.reply(s).map(|r| (get_i16(&r, 12), get_i16(&r, 14)))
    }

    pub fn set_input_focus(&mut self, w: Window, time: u32) {
        let mut b = Vec::new();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&time.to_le_bytes());
        self.request(42, 2, &b); // revert to parent
    }

    /// A cursor from the standard cursor font (XC_* glyph numbers).
    pub fn font_cursor(&mut self, glyph: u16) -> u32 {
        let font = self.gen_id();
        let mut b = Vec::new();
        b.extend_from_slice(&font.to_le_bytes());
        b.extend_from_slice(&6u16.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(b"cursor");
        self.request(45, 0, &b);
        let cursor = self.gen_id();
        let mut b = Vec::new();
        for v in [cursor, font, font] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&glyph.to_le_bytes());
        b.extend_from_slice(&(glyph + 1).to_le_bytes());
        for v in [0u16, 0, 0, 0xFFFF, 0xFFFF, 0xFFFF] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        self.request(94, 0, &b);
        self.request(46, 0, &font.to_le_bytes()); // CloseFont
        cursor
    }

    pub fn create_gc(&mut self, drawable: Window) -> u32 {
        let gc = self.gen_id();
        let mut b = Vec::new();
        b.extend_from_slice(&gc.to_le_bytes());
        b.extend_from_slice(&drawable.to_le_bytes());
        // GCGraphicsExposures off.
        b.extend_from_slice(&0x1_0000u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        self.request(55, 0, &b);
        gc
    }

    /// Copy 32-bit pixels (0xAARRGGBB, top-down) to a drawable of `depth`,
    /// in strips that fit the server's request size limit.
    #[allow(clippy::too_many_arguments)]
    pub fn put_image(
        &mut self,
        drawable: Window,
        gc: u32,
        depth: u8,
        px: &[u32],
        w: usize,
        h: usize,
        x: i32,
        y: i32,
    ) {
        if w == 0 || h == 0 || px.len() < w * h {
            return;
        }
        let row = w * 4;
        let rows_per = ((self.setup.max_request_bytes.saturating_sub(64)) / row).clamp(1, h);
        let mut top = 0;
        while top < h {
            let n = rows_per.min(h - top);
            let mut b = Vec::with_capacity(20 + n * row);
            b.extend_from_slice(&drawable.to_le_bytes());
            b.extend_from_slice(&gc.to_le_bytes());
            b.extend_from_slice(&(w as u16).to_le_bytes());
            b.extend_from_slice(&(n as u16).to_le_bytes());
            b.extend_from_slice(&(x as i16).to_le_bytes());
            b.extend_from_slice(&((y + top as i32) as i16).to_le_bytes());
            b.push(0); // left pad
            b.push(depth);
            b.extend_from_slice(&[0, 0]);
            for p in &px[top * w..(top + n) * w] {
                b.extend_from_slice(&p.to_le_bytes());
            }
            self.request(72, 2, &b);
            top += n;
        }
    }

    /// Set a window's bounding shape (Shape extension) to `rects`.
    pub fn set_shape(&mut self, w: Window, rects: &[(i16, i16, u16, u16)]) {
        let Some(major) = self.shape else { return };
        let mut b = vec![0, 0, 0, 0]; // Set, Bounding, UnSorted, pad
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 0]); // offset
        for (x, y, rw, rh) in rects {
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
            b.extend_from_slice(&rw.to_le_bytes());
            b.extend_from_slice(&rh.to_le_bytes());
        }
        self.request(major, 1, &b); // ShapeRectangles
    }

    pub fn bell(&mut self) {
        self.request(104, 0, &[]);
    }

    pub fn query_extension(&mut self, name: &str) -> Option<(u8, u8)> {
        let mut b = Vec::new();
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(name.as_bytes());
        let s = self.request(98, 0, &b);
        let r = self.reply(s)?;
        (r[8] != 0).then_some((r[9], r[10]))
    }

    /// The monitors (RandR 1.5), or the whole screen as one.
    pub fn monitors(&mut self) -> Vec<Monitor> {
        let screen = Monitor {
            x: 0,
            y: 0,
            w: i32::from(self.setup.width),
            h: i32::from(self.setup.height),
            primary: true,
        };
        let Some(major) = self.randr else {
            return vec![screen];
        };
        let mut b = Vec::new();
        b.extend_from_slice(&self.setup.root.to_le_bytes());
        b.extend_from_slice(&[1, 0, 0, 0]);
        let s = self.request(major, 42, &b);
        let Some(r) = self.reply(s) else {
            return vec![screen];
        };
        let n = get_u32(&r, 12) as usize;
        let mut at = 32;
        let mut list = Vec::new();
        for _ in 0..n.min(32) {
            if at + 24 > r.len() {
                break;
            }
            let outputs = usize::from(get_u16(&r, at + 6));
            list.push(Monitor {
                x: get_i16(&r, at + 8),
                y: get_i16(&r, at + 10),
                w: i32::from(get_u16(&r, at + 12)),
                h: i32::from(get_u16(&r, at + 14)),
                primary: r[at + 4] != 0,
            });
            at += 24 + 4 * outputs;
        }
        if list.is_empty() { vec![screen] } else { list }
    }

    fn load_keymap(&mut self) {
        let (min, max) = (self.setup.min_keycode, self.setup.max_keycode);
        if max < min {
            return;
        }
        let count = max - min + 1;
        let s = self.request(101, 0, &[min, count, 0, 0]);
        if let Some(r) = self.reply(s) {
            self.per_code = usize::from(r[1]);
            self.keysyms = r[32..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect();
        }
    }

    pub fn reload_keymap(&mut self) {
        self.load_keymap();
    }

    /// The keysym for a keycode, the shifted one when `shift`.
    pub fn keysym(&self, code: u8, shift: bool) -> u32 {
        if self.per_code == 0 || code < self.setup.min_keycode {
            return 0;
        }
        let base = usize::from(code - self.setup.min_keycode) * self.per_code;
        let plain = self.keysyms.get(base).copied().unwrap_or(0);
        let shifted = if self.per_code > 1 {
            self.keysyms.get(base + 1).copied().unwrap_or(0)
        } else {
            0
        };
        if shift && shifted != 0 {
            shifted
        } else if shift && (0x61..=0x7A).contains(&plain) {
            plain - 0x20
        } else {
            plain
        }
    }
}

fn read_exact(s: &mut Stream, buf: &mut [u8]) -> Option<()> {
    let mut at = 0;
    while at < buf.len() {
        match s.read(&mut buf[at..]) {
            Ok(0) => return None,
            Ok(n) => at += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(())
}

fn parse_setup(b: &[u8]) -> Option<Setup> {
    let id_base = get_u32(b, 4);
    let id_mask = get_u32(b, 8);
    let vendor_len = usize::from(get_u16(b, 16));
    let max_req = usize::from(get_u16(b, 18)) * 4;
    let screens = b[20];
    let formats = usize::from(b[21]);
    let (min_keycode, max_keycode) = (b[26], b[27]);
    let mut at = 32 + vendor_len + pad4(vendor_len);
    let mut bpp24 = 32;
    for i in 0..formats {
        let f = b.get(at + 8 * i..at + 8 * i + 8)?;
        if f[0] == 24 {
            bpp24 = f[1];
        }
    }
    at += 8 * formats;
    if screens == 0 {
        return None;
    }
    let s = b.get(at..at + 40)?;
    let root = get_u32(s, 0);
    let width = get_u16(s, 20);
    let height = get_u16(s, 22);
    let root_depth = s[38];
    let depths = usize::from(s[39]);
    at += 40;
    let mut argb = None;
    for _ in 0..depths {
        let d = b.get(at..at + 8)?;
        let depth = d[0];
        let visuals = usize::from(get_u16(d, 2));
        at += 8;
        for v in 0..visuals {
            let e = b.get(at + 24 * v..at + 24 * v + 24)?;
            // TrueColor, 8 bits per channel, 0xAARRGGBB.
            if depth == 32
                && argb.is_none()
                && e[4] == 4
                && get_u32(e, 8) == 0xFF_0000
                && get_u32(e, 12) == 0xFF00
                && get_u32(e, 16) == 0xFF
            {
                argb = Some(Visual {
                    id: get_u32(e, 0),
                    depth: 32,
                });
            }
        }
        at += 24 * visuals;
    }
    // 24-bit pixels must travel as 32 bits each (ZPixmap), as everywhere today.
    (bpp24 == 32).then_some(Setup {
        root,
        root_depth,
        width,
        height,
        min_keycode,
        max_keycode,
        max_request_bytes: max_req.max(4096),
        argb,
        id_base,
        id_mask,
    })
}

fn decode_event(p: &[u8]) -> Event {
    let code = p[0] & 0x7F;
    let u32_ = |i| get_u32(p, i);
    match code {
        2 | 3 => Event::Key {
            press: code == 2,
            code: p[1],
            state: get_u16(p, 28),
            window: u32_(12),
            time: u32_(4),
        },
        4 | 5 => Event::Button {
            press: code == 4,
            button: p[1],
            state: get_u16(p, 28),
            window: u32_(12),
            x: get_i16(p, 24),
            y: get_i16(p, 26),
            root_x: get_i16(p, 20),
            root_y: get_i16(p, 22),
            time: u32_(4),
        },
        6 => Event::Motion {
            window: u32_(12),
            x: get_i16(p, 24),
            y: get_i16(p, 26),
            state: get_u16(p, 28),
            time: u32_(4),
        },
        7 | 8 => Event::Crossing {
            enter: code == 7,
            window: u32_(12),
            mode: p[30],
            time: u32_(4),
        },
        9 | 10 => Event::Focus {
            gained: code == 9,
            window: u32_(4),
            mode: p[8],
            detail: p[1],
        },
        12 => Event::Expose {
            window: u32_(4),
            count: get_u16(p, 16),
        },
        17 | 18 | 19 | 21 => Event::Structure,
        22 => Event::Configure {
            window: u32_(8),
            x: get_i16(p, 16),
            y: get_i16(p, 18),
            w: i32::from(get_u16(p, 20)),
            h: i32::from(get_u16(p, 22)),
        },
        28 => Event::Property {
            window: u32_(4),
            atom: u32_(8),
        },
        29 => Event::SelectionClear {
            selection: u32_(12),
        },
        30 => Event::SelectionRequest {
            time: u32_(4),
            requestor: u32_(12),
            selection: u32_(16),
            target: u32_(20),
            property: u32_(24),
        },
        31 => Event::SelectionNotify {
            requestor: u32_(8),
            property: u32_(20),
        },
        33 => Event::Client {
            window: u32_(4),
            kind: u32_(8),
            data: [u32_(12), u32_(16), u32_(20), u32_(24), u32_(28)],
        },
        34 => Event::Mapping,
        _ => Event::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names() {
        assert_eq!(parse_display(":0"), Some((String::new(), 0)));
        assert_eq!(parse_display(":1.0"), Some((String::new(), 1)));
        assert_eq!(parse_display("unix:2"), Some(("unix".into(), 2)));
        assert_eq!(parse_display("host:10.1"), Some(("host".into(), 10)));
        assert_eq!(parse_display("nonsense"), None);
    }

    #[test]
    fn xauthority() {
        let entry = |family: u16, addr: &[u8], num: &[u8], cookie: &[u8]| {
            let mut v = family.to_be_bytes().to_vec();
            for f in [addr, num, b"MIT-MAGIC-COOKIE-1".as_slice(), cookie] {
                v.extend_from_slice(&(f.len() as u16).to_be_bytes());
                v.extend_from_slice(f);
            }
            v
        };
        let mut file = entry(256, b"otherhost", b"0", b"aaaa");
        file.extend(entry(256, b"myhost", b"1", b"bbbb"));
        file.extend(entry(256, b"myhost", b"0", b"cccc"));
        assert_eq!(find_cookie(&file, "myhost", 0), Some(b"cccc".to_vec()));
        assert_eq!(find_cookie(&file, "myhost", 1), Some(b"bbbb".to_vec()));
        assert_eq!(find_cookie(&file, "nohost", 0), Some(b"aaaa".to_vec()));
        assert_eq!(find_cookie(&file[..10], "myhost", 0), None);
    }

    #[test]
    fn events_decode() {
        let mut p = [0u8; 32];
        p[0] = 4;
        p[1] = 3;
        p[12..16].copy_from_slice(&77u32.to_le_bytes());
        p[24..26].copy_from_slice(&(-5i16).to_le_bytes());
        p[26..28].copy_from_slice(&9u16.to_le_bytes());
        match decode_event(&p) {
            Event::Button {
                press,
                button,
                window,
                x,
                y,
                ..
            } => assert_eq!((press, button, window, x, y), (true, 3, 77, -5, 9)),
            e => panic!("{e:?}"),
        }
        let m = Conn::client_message(5, 6, [1, 2, 3, 4, 5]);
        match decode_event(&m) {
            Event::Client { window, kind, data } => {
                assert_eq!((window, kind, data), (5, 6, [1, 2, 3, 4, 5]))
            }
            e => panic!("{e:?}"),
        }
    }
}
