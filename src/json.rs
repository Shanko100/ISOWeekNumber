//! A small, strict JSON reader (RFC 8259) with depth and size limits, and the
//! writer for settings.json. Shared with build.rs, which uses it to convert the
//! bundled data files; so it depends on nothing but `std`.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    /// Whether the number was written without fraction or exponent.
    Int(i64),
    Str(String),
    Array(Vec<Value>),
    /// Keys in file order; duplicates keep the last value, as Python's json.
    Object(Vec<(String, Value)>),
}

// Some accessors are used only by build.rs, which shares this module.
#[allow(dead_code)]
impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Value::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        if let Value::Int(i) = self {
            Some(*i)
        } else {
            None
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        if let Value::Array(a) = self {
            Some(a)
        } else {
            None
        }
    }
    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        if let Value::Object(o) = self {
            Some(o)
        } else {
            None
        }
    }
}

/// Nesting limit: settings are flat; the data files nest four deep.
const MAX_DEPTH: usize = 16;

pub fn parse(text: &str) -> Option<Value> {
    let mut p = Parser {
        b: text.as_bytes(),
        i: 0,
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i == p.b.len() { Some(v) } else { None }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Some(&c) = self.b.get(self.i) {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, lit: &[u8]) -> Option<()> {
        if self.b.get(self.i..self.i + lit.len())? == lit {
            self.i += lit.len();
            Some(())
        } else {
            None
        }
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        match *self.b.get(self.i)? {
            b'n' => self.eat(b"null").map(|_| Value::Null),
            b't' => self.eat(b"true").map(|_| Value::Bool(true)),
            b'f' => self.eat(b"false").map(|_| Value::Bool(false)),
            b'"' => self.string().map(Value::Str),
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Some(Value::Array(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match *self.b.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Some(Value::Array(items));
                        }
                        _ => return None,
                    }
                }
            }
            b'{' => {
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Some(Value::Object(members));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        return None;
                    }
                    let key = self.string()?;
                    self.ws();
                    self.eat(b":")?;
                    self.ws();
                    let value = self.value(depth + 1)?;
                    members.push((key, value));
                    self.ws();
                    match *self.b.get(self.i)? {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Some(Value::Object(members));
                        }
                        _ => return None,
                    }
                }
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match *self.b.get(self.i)? {
            b'0' => self.i += 1,
            b'1'..=b'9' => {
                while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            }
            _ => return None,
        }
        let mut integral = true;
        if self.b.get(self.i) == Some(&b'.') {
            integral = false;
            self.i += 1;
            if !matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                return None;
            }
            while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            integral = false;
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                return None;
            }
            while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).ok()?;
        if integral && let Ok(i) = text.parse::<i64>() {
            return Some(Value::Int(i));
        }
        let n: f64 = text.parse().ok()?;
        n.is_finite().then_some(Value::Number(n))
    }

    fn hex4(&mut self) -> Option<u32> {
        let s = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        let v = u32::from_str_radix(s, 16).ok()?;
        self.i += 4;
        Some(v)
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(&c) = self.b.get(self.i) {
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.b[start..self.i]).ok()?);
            match *self.b.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.i += 1;
                    let esc = *self.b.get(self.i)?;
                    self.i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                self.eat(b"\\u")?;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code)?);
                        }
                        _ => return None,
                    }
                }
                _ => return None, // raw control character
            }
        }
    }
}

/// A JSON string literal the way Python's json.dumps writes it
/// (ensure_ascii=True).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7E => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Flat object values settings.json holds.
pub enum Flat<'a> {
    Str(&'a str),
    Bool(bool),
    Null,
}

/// `{ "k": v, ... }` exactly as Python's json.dump(obj, f, indent=2).
pub fn write_flat_object(members: &[(&str, Flat<'_>)]) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in members.iter().enumerate() {
        out.push_str(if index == 0 { "\n  " } else { ",\n  " });
        out.push_str(&quote(key));
        out.push_str(": ");
        match value {
            Flat::Str(s) => out.push_str(&quote(s)),
            Flat::Bool(true) => out.push_str("true"),
            Flat::Bool(false) => out.push_str("false"),
            Flat::Null => out.push_str("null"),
        }
    }
    out.push_str(if members.is_empty() { "}" } else { "\n}" });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rejects() {
        let v = parse(r#"{"a": [1, -2.5e1, true, null, "xé😀"], "a": 3}"#).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Int(3)));
        for bad in [
            "",
            "{",
            "[1,]",
            "01",
            "\"\x01\"",
            "{\"a\" 1}",
            "1 2",
            "nul",
            "1.",
            "-",
        ] {
            assert!(parse(bad).is_none(), "{bad:?}");
        }
        let deep = "[".repeat(40) + &"]".repeat(40);
        assert!(parse(&deep).is_none());
    }

    #[test]
    fn writes_like_python() {
        let s = write_flat_object(&[
            ("A", Flat::Str("#FFFFFF")),
            ("B", Flat::Bool(true)),
            ("C", Flat::Null),
        ]);
        assert_eq!(
            s,
            "{\n  \"A\": \"#FFFFFF\",\n  \"B\": true,\n  \"C\": null\n}"
        );
        assert_eq!(quote("é\u{1F600}"), "\"\\u00e9\\ud83d\\ude00\"");
    }
}
