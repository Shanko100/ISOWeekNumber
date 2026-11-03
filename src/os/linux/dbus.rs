//! A minimal D-Bus client: the session bus connection (SASL EXTERNAL), the
//! wire format for the types the tray icon needs, and message framing.
//! Incoming messages are untrusted: lengths, nesting and signatures are
//! bounded and checked, and anything malformed is dropped.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

const MAX_MESSAGE: usize = 1 << 20;
const MAX_DEPTH: u32 = 16;

pub const METHOD_CALL: u8 = 1;
pub const METHOD_RETURN: u8 = 2;
pub const ERROR: u8 = 3;
pub const SIGNAL: u8 = 4;
/// Flag: the caller wants no reply.
pub const NO_REPLY: u8 = 1;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    Str(String),
    Path(String),
    Sig(String),
    Variant(Box<Value>),
    /// Element signature and elements.
    Array(String, Vec<Value>),
    Struct(Vec<Value>),
    Dict(Box<Value>, Box<Value>),
    /// A byte array ("ay") held as bytes: icon pixels, sent as they are
    /// instead of one Value per byte.
    Bytes(Vec<u8>),
}

impl Value {
    pub fn signature(&self) -> String {
        match self {
            Value::Byte(_) => "y".into(),
            Value::Bool(_) => "b".into(),
            Value::I32(_) => "i".into(),
            Value::U32(_) => "u".into(),
            Value::I64(_) => "x".into(),
            Value::Str(_) => "s".into(),
            Value::Path(_) => "o".into(),
            Value::Sig(_) => "g".into(),
            Value::Variant(_) => "v".into(),
            Value::Array(e, _) => format!("a{e}"),
            Value::Struct(v) => format!("({})", v.iter().map(Value::signature).collect::<String>()),
            Value::Dict(k, v) => format!("{{{}{}}}", k.signature(), v.signature()),
            Value::Bytes(_) => "ay".into(),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::Path(s) | Value::Sig(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::I32(v) => Some(*v),
            _ => None,
        }
    }

    /// A string-keyed dictionary entry {sv}.
    pub fn entry(key: &str, v: Value) -> Value {
        Value::Dict(
            Box::new(Value::Str(key.into())),
            Box::new(Value::Variant(Box::new(v))),
        )
    }
}

fn align_of(t: u8) -> usize {
    match t {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

fn pad(buf: &mut Vec<u8>, a: usize) {
    while !buf.len().is_multiple_of(a) {
        buf.push(0);
    }
}

pub fn write(buf: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Byte(b) => buf.push(*b),
        Value::Bool(b) => {
            pad(buf, 4);
            buf.extend_from_slice(&u32::from(*b).to_le_bytes());
        }
        Value::I32(i) => {
            pad(buf, 4);
            buf.extend_from_slice(&i.to_le_bytes());
        }
        Value::U32(u) => {
            pad(buf, 4);
            buf.extend_from_slice(&u.to_le_bytes());
        }
        Value::I64(i) => {
            pad(buf, 8);
            buf.extend_from_slice(&i.to_le_bytes());
        }
        Value::Str(s) | Value::Path(s) => {
            pad(buf, 4);
            buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
        }
        Value::Sig(s) => {
            buf.push(s.len() as u8);
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
        }
        Value::Variant(inner) => {
            write(buf, &Value::Sig(inner.signature()));
            write(buf, inner);
        }
        Value::Array(elem, items) => {
            pad(buf, 4);
            let len_at = buf.len();
            buf.extend_from_slice(&[0; 4]);
            pad(buf, align_of(elem.as_bytes()[0]));
            let start = buf.len();
            for i in items {
                write(buf, i);
            }
            let len = (buf.len() - start) as u32;
            buf[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
        }
        Value::Struct(fields) => {
            pad(buf, 8);
            for f in fields {
                write(buf, f);
            }
        }
        Value::Dict(k, val) => {
            pad(buf, 8);
            write(buf, k);
            write(buf, val);
        }
        Value::Bytes(b) => {
            pad(buf, 4);
            buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
            buf.extend_from_slice(b);
        }
    }
}

/// The length of the first complete type in a signature.
fn single_type_len(sig: &[u8]) -> Option<usize> {
    match *sig.first()? {
        b'a' => Some(1 + single_type_len(&sig[1..])?),
        open @ (b'(' | b'{') => {
            let close = if open == b'(' { b')' } else { b'}' };
            let mut i = 1;
            while *sig.get(i)? != close {
                i += single_type_len(&sig[i..])?;
            }
            Some(i + 1)
        }
        b'y' | b'b' | b'n' | b'q' | b'i' | b'u' | b'x' | b't' | b'd' | b's' | b'o' | b'g'
        | b'v' | b'h' => Some(1),
        _ => None,
    }
}

/// Split a signature into its complete types.
pub fn split_signature(sig: &str) -> Option<Vec<String>> {
    let b = sig.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let n = single_type_len(&b[i..])?;
        out.push(sig[i..i + n].to_string());
        i += n;
    }
    Some(out)
}

pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0 }
    }

    fn align(&mut self, a: usize) -> Option<()> {
        let p = self.pos.div_ceil(a) * a;
        (p <= self.b.len()).then(|| self.pos = p)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }

    fn u32(&mut self) -> Option<u32> {
        self.align(4)?;
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn read(&mut self, sig: &str, depth: u32) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        let b = sig.as_bytes();
        Some(match *b.first()? {
            b'y' => Value::Byte(*self.take(1)?.first()?),
            b'b' => Value::Bool(self.u32()? != 0),
            b'i' => Value::I32(self.u32()? as i32),
            b'u' => Value::U32(self.u32()?),
            b'h' => Value::U32(self.u32()?),
            b'n' | b'q' => {
                self.align(2)?;
                let v = u16::from_le_bytes(self.take(2)?.try_into().ok()?);
                Value::I32(i32::from(v as i16))
            }
            b'x' | b't' | b'd' => {
                self.align(8)?;
                Value::I64(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
            }
            c @ (b's' | b'o') => {
                let n = self.u32()? as usize;
                let s = std::str::from_utf8(self.take(n)?).ok()?.to_string();
                self.take(1)?;
                if c == b's' {
                    Value::Str(s)
                } else {
                    Value::Path(s)
                }
            }
            b'g' => {
                let n = usize::from(*self.take(1)?.first()?);
                let s = std::str::from_utf8(self.take(n)?).ok()?.to_string();
                self.take(1)?;
                Value::Sig(s)
            }
            b'v' => {
                let Value::Sig(inner) = self.read("g", depth + 1)? else {
                    return None;
                };
                if single_type_len(inner.as_bytes())? != inner.len() {
                    return None;
                }
                Value::Variant(Box::new(self.read(&inner, depth + 1)?))
            }
            b'a' => {
                let elem = &sig[1..1 + single_type_len(&b[1..])?];
                let n = self.u32()? as usize;
                if n > MAX_MESSAGE {
                    return None;
                }
                self.align(align_of(elem.as_bytes()[0]))?;
                let end = self.pos.checked_add(n)?;
                if end > self.b.len() {
                    return None;
                }
                let mut items = Vec::new();
                while self.pos < end {
                    items.push(self.read(elem, depth + 1)?);
                }
                Value::Array(elem.to_string(), items)
            }
            b'(' => {
                self.align(8)?;
                let inner = &sig[1..single_type_len(b)? - 1];
                let mut fields = Vec::new();
                for t in split_signature(inner)? {
                    fields.push(self.read(&t, depth + 1)?);
                }
                Value::Struct(fields)
            }
            b'{' => {
                self.align(8)?;
                let inner = split_signature(&sig[1..single_type_len(b)? - 1])?;
                if inner.len() != 2 {
                    return None;
                }
                let k = self.read(&inner[0], depth + 1)?;
                let v = self.read(&inner[1], depth + 1)?;
                Value::Dict(Box::new(k), Box::new(v))
            }
            _ => return None,
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct Message {
    pub kind: u8,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    pub body: Vec<Value>,
}

impl Message {
    pub fn call(dest: &str, path: &str, iface: &str, member: &str, body: Vec<Value>) -> Message {
        Message {
            kind: METHOD_CALL,
            destination: Some(dest.into()),
            path: Some(path.into()),
            interface: Some(iface.into()),
            member: Some(member.into()),
            body,
            ..Message::default()
        }
    }

    pub fn signal(path: &str, iface: &str, member: &str, body: Vec<Value>) -> Message {
        Message {
            kind: SIGNAL,
            path: Some(path.into()),
            interface: Some(iface.into()),
            member: Some(member.into()),
            body,
            ..Message::default()
        }
    }

    pub fn reply_to(call: &Message, body: Vec<Value>) -> Message {
        Message {
            kind: METHOD_RETURN,
            flags: NO_REPLY,
            reply_serial: Some(call.serial),
            destination: call.sender.clone(),
            body,
            ..Message::default()
        }
    }

    pub fn error_to(call: &Message, name: &str, text: &str) -> Message {
        Message {
            kind: ERROR,
            flags: NO_REPLY,
            reply_serial: Some(call.serial),
            destination: call.sender.clone(),
            error: Some(name.into()),
            body: vec![Value::Str(text.into())],
            ..Message::default()
        }
    }

    pub fn encode(&self, serial: u32) -> Vec<u8> {
        let mut body = Vec::new();
        for v in &self.body {
            write(&mut body, v);
        }
        let sig: String = self.body.iter().map(Value::signature).collect();
        let mut fields = Vec::new();
        let mut field = |code: u8, v: Value| {
            fields.push(Value::Struct(vec![
                Value::Byte(code),
                Value::Variant(Box::new(v)),
            ]));
        };
        if let Some(p) = &self.path {
            field(1, Value::Path(p.clone()));
        }
        if let Some(i) = &self.interface {
            field(2, Value::Str(i.clone()));
        }
        if let Some(m) = &self.member {
            field(3, Value::Str(m.clone()));
        }
        if let Some(e) = &self.error {
            field(4, Value::Str(e.clone()));
        }
        if let Some(r) = self.reply_serial {
            field(5, Value::U32(r));
        }
        if let Some(d) = &self.destination {
            field(6, Value::Str(d.clone()));
        }
        if !sig.is_empty() {
            field(8, Value::Sig(sig));
        }
        let mut out = vec![b'l', self.kind, self.flags, 1];
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&serial.to_le_bytes());
        write(&mut out, &Value::Array("(yv)".into(), fields));
        pad(&mut out, 8);
        out.extend_from_slice(&body);
        out
    }

    /// One message from the front of `b`: (message, bytes used), None when
    /// incomplete, Err when malformed.
    pub fn decode(b: &[u8]) -> Result<Option<(Message, usize)>, ()> {
        if b.len() < 16 {
            return Ok(None);
        }
        if b[0] != b'l' || b[3] != 1 {
            return Err(());
        }
        let body_len = u32::from_le_bytes(b[4..8].try_into().map_err(|_| ())?) as usize;
        let fields_len = u32::from_le_bytes(b[12..16].try_into().map_err(|_| ())?) as usize;
        if body_len > MAX_MESSAGE || fields_len > MAX_MESSAGE {
            return Err(());
        }
        let head = (16 + fields_len).div_ceil(8) * 8;
        let total = head + body_len;
        if b.len() < total {
            return Ok(None);
        }
        let mut m = Message {
            kind: b[1],
            flags: b[2],
            serial: u32::from_le_bytes(b[8..12].try_into().map_err(|_| ())?),
            ..Message::default()
        };
        let mut r = Reader::new(&b[..head]);
        r.pos = 12;
        let Some(Value::Array(_, fields)) = r.read("a(yv)", 0) else {
            return Err(());
        };
        let mut sig = String::new();
        for f in fields {
            let Value::Struct(f) = f else { continue };
            let (Some(Value::Byte(code)), Some(Value::Variant(v))) = (f.first(), f.get(1)) else {
                continue;
            };
            match (code, v.as_ref()) {
                (1, Value::Path(p)) => m.path = Some(p.clone()),
                (2, Value::Str(s)) => m.interface = Some(s.clone()),
                (3, Value::Str(s)) => m.member = Some(s.clone()),
                (4, Value::Str(s)) => m.error = Some(s.clone()),
                (5, Value::U32(u)) => m.reply_serial = Some(*u),
                (6, Value::Str(s)) => m.destination = Some(s.clone()),
                (7, Value::Str(s)) => m.sender = Some(s.clone()),
                (8, Value::Sig(s)) => sig = s.clone(),
                _ => {}
            }
        }
        let mut r = Reader::new(&b[head..total]);
        for t in split_signature(&sig).ok_or(())? {
            m.body.push(r.read(&t, 0).ok_or(())?);
        }
        Ok(Some((m, total)))
    }
}

pub struct Bus {
    stream: UnixStream,
    inbuf: Vec<u8>,
    pending: std::collections::VecDeque<Message>,
    serial: u32,
    pub unique_name: String,
}

/// The session bus address -> a connected socket.
fn connect_address(addr: &str) -> Option<UnixStream> {
    for part in addr.split(';') {
        let Some(rest) = part.strip_prefix("unix:") else {
            continue;
        };
        for kv in rest.split(',') {
            if let Some(p) = kv.strip_prefix("path=")
                && let Ok(s) = UnixStream::connect(unescape(p))
            {
                return Some(s);
            }
            if let Some(name) = kv.strip_prefix("abstract=") {
                use std::os::linux::net::SocketAddrExt;
                if let Ok(a) =
                    std::os::unix::net::SocketAddr::from_abstract_name(unescape(name).as_bytes())
                    && let Ok(s) = UnixStream::connect_addr(&a)
                {
                    return Some(s);
                }
            }
        }
    }
    None
}

/// D-Bus address values escape bytes as %xx.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Bus {
    pub fn session() -> Option<Bus> {
        let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().or_else(|| {
            let run = std::env::var("XDG_RUNTIME_DIR").ok()?;
            Some(format!("unix:path={run}/bus"))
        })?;
        Bus::connect(&addr)
    }

    pub fn connect(addr: &str) -> Option<Bus> {
        let mut stream = connect_address(addr)?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .ok()?;
        let uid = super::super::unix::sys::uid().to_string();
        let hex: String = uid.bytes().map(|b| format!("{b:02x}")).collect();
        stream
            .write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes())
            .ok()?;
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while !line.ends_with(b"\r\n") && line.len() < 512 {
            if stream.read(&mut byte).ok()? == 0 {
                return None;
            }
            line.push(byte[0]);
        }
        if !line.starts_with(b"OK ") {
            return None;
        }
        stream.write_all(b"BEGIN\r\n").ok()?;
        let mut bus = Bus {
            stream,
            inbuf: Vec::new(),
            pending: std::collections::VecDeque::new(),
            serial: 0,
            unique_name: String::new(),
        };
        let hello = Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
            Vec::new(),
        );
        let reply = bus.call(&hello)?;
        bus.unique_name = reply.body.first()?.as_str()?.to_string();
        bus.stream.set_nonblocking(true).ok()?;
        Some(bus)
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub fn send(&mut self, m: &Message) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        let bytes = m.encode(self.serial);
        let _ = self.stream.set_nonblocking(false);
        let _ = self.stream.write_all(&bytes);
        let _ = self.stream.set_nonblocking(true);
        self.serial
    }

    /// Send a call and wait (up to 5 s) for its reply; other messages
    /// arriving meanwhile stay queued for `poll`.
    pub fn call(&mut self, m: &Message) -> Option<Message> {
        let serial = self.send(m);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut result = None;
        while std::time::Instant::now() < deadline {
            let _ = self.stream.set_nonblocking(true);
            self.fill();
            while let Some(msg) = self.next_buffered() {
                if msg.reply_serial == Some(serial)
                    && (msg.kind == METHOD_RETURN || msg.kind == ERROR)
                {
                    result = Some(msg);
                } else {
                    self.pending.push_back(msg);
                }
            }
            if result.is_some() {
                break;
            }
            super::super::unix::sys::wait_readable(&[self.fd()], 200);
        }
        result.filter(|r| r.kind == METHOD_RETURN)
    }

    fn fill(&mut self) {
        let mut buf = [0u8; 8192];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.inbuf.extend_from_slice(&buf[..n]);
                    if self.inbuf.len() > 4 * MAX_MESSAGE {
                        self.inbuf.clear();
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }

    fn next_buffered(&mut self) -> Option<Message> {
        match Message::decode(&self.inbuf) {
            Ok(Some((m, used))) => {
                self.inbuf.drain(..used);
                Some(m)
            }
            Ok(None) => None,
            Err(()) => {
                // Unparseable: drop the buffer rather than desynchronise.
                self.inbuf.clear();
                None
            }
        }
    }

    /// Messages that have arrived.
    pub fn poll(&mut self) -> Vec<Message> {
        self.fill();
        let mut out: Vec<Message> = self.pending.drain(..).collect();
        while let Some(m) = self.next_buffered() {
            out.push(m);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        assert_eq!(
            split_signature("a(iiay)sa{sv}(ia{sv}av)").unwrap(),
            vec!["a(iiay)", "s", "a{sv}", "(ia{sv}av)"]
        );
        assert!(split_signature("a").is_none());
        assert!(split_signature("(ii").is_none());
        let v = Value::Struct(vec![
            Value::I32(1),
            Value::Array(
                "{sv}".into(),
                vec![Value::entry("label", Value::Str("x".into()))],
            ),
        ]);
        assert_eq!(v.signature(), "(ia{sv})");
    }

    #[test]
    fn message_round_trip() {
        let pixmap = Value::Array(
            "(iiay)".into(),
            vec![Value::Struct(vec![
                Value::I32(2),
                Value::I32(1),
                Value::Array("y".into(), vec![Value::Byte(1); 8]),
            ])],
        );
        let mut m = Message::call(
            "a.b",
            "/x",
            "a.b.C",
            "M",
            vec![Value::Str("hi".into()), pixmap.clone(), Value::Bool(true)],
        );
        m.sender = None;
        let bytes = m.encode(7);
        let (back, used) = Message::decode(&bytes).unwrap().unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(back.serial, 7);
        assert_eq!(back.member.as_deref(), Some("M"));
        assert_eq!(back.body, m.body);
        // Bytes go out exactly as an array of Byte values does.
        let mut a = Vec::new();
        let mut b = Vec::new();
        write(
            &mut a,
            &Value::Array("y".into(), vec![Value::Byte(1), Value::Byte(2)]),
        );
        write(&mut b, &Value::Bytes(vec![1, 2]));
        assert_eq!(a, b);
        assert_eq!(Value::Bytes(Vec::new()).signature(), "ay");
        assert!(
            Message::decode(&bytes[..bytes.len() - 1])
                .unwrap()
                .is_none()
        );
        let mut bad = bytes.clone();
        bad[0] = b'B';
        assert!(Message::decode(&bad).is_err());
        // A variant holding two types is rejected.
        let mut r = Reader::new(&[2, b'i', b'i', 0, 0, 0, 0, 0]);
        assert!(r.read("v", 0).is_none());
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let sig = "a".repeat(40) + "y";
        let mut b = Vec::new();
        for _ in 0..40 {
            b.extend_from_slice(&4u32.to_le_bytes());
        }
        let mut r = Reader::new(&b);
        assert!(r.read(&sig, 0).is_none());
    }

    #[test]
    fn address_escapes() {
        assert_eq!(unescape("/run/user/1000/bus"), "/run/user/1000/bus");
        assert_eq!(unescape("/tmp/a%20b"), "/tmp/a b");
    }
}
