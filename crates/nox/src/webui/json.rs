//! A small JSON value with a writer and a parser (RFC 8259), enough for the WebAPI's replies,
//! `setPreferences` and the Web UI's own state file. Objects keep their insertion order.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    /// Appends (or replaces) `key` in an object; does nothing to other values.
    pub fn set(&mut self, key: &str, v: impl Into<Json>) {
        if let Json::Obj(m) = self {
            let v = v.into();
            match m.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v,
                None => m.push((key.to_owned(), v)),
            }
        }
    }

    pub fn with(mut self, key: &str, v: impl Into<Json>) -> Json {
        self.set(key, v);
        self
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Integers, and floats with no fraction.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Json::Int(i) => Some(i),
            Json::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(f as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Json::Int(i) => Some(i as f64),
            Json::Float(f) => Some(f),
            _ => None,
        }
    }

    /// Booleans; also `0` / `1` and `"true"` / `"false"`, which some clients send.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            Json::Int(0) => Some(false),
            Json::Int(1) => Some(true),
            Json::Str(s) if s == "true" => Some(true),
            Json::Str(s) if s == "false" => Some(false),
            _ => None,
        }
    }

    pub fn entries(&self) -> &[(String, Json)] {
        match self {
            Json::Obj(m) => m,
            _ => &[],
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Json::Float(f) => write_float(*f, out),
            Json::Str(s) => write_str(s, out),
            Json::Arr(v) => {
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Json::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_float(f: f64, out: &mut String) {
    if !f.is_finite() {
        // JSON has no infinities; callers clamp the values that can be infinite.
        out.push_str("null");
    } else {
        // Rust prints the shortest text that reads back as the same double, without an
        // exponent; a fraction keeps an integral value a float when read back.
        let start = out.len();
        let _ = write!(out, "{f}");
        if !out[start..].contains(['.', 'e', 'E']) {
            out.push_str(".0");
        }
    }
}

pub fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `<` keeps a string from closing a `<script>` element it is embedded in.
            c if (c as u32) < 0x20 || c == '<' || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}
impl From<i64> for Json {
    fn from(i: i64) -> Json {
        Json::Int(i)
    }
}
impl From<i32> for Json {
    fn from(i: i32) -> Json {
        Json::Int(i as i64)
    }
}
impl From<u32> for Json {
    fn from(i: u32) -> Json {
        Json::Int(i as i64)
    }
}
impl From<u64> for Json {
    fn from(i: u64) -> Json {
        Json::Int(i.min(i64::MAX as u64) as i64)
    }
}
impl From<usize> for Json {
    fn from(i: usize) -> Json {
        Json::from(i as u64)
    }
}
impl From<f64> for Json {
    fn from(f: f64) -> Json {
        Json::Float(f)
    }
}
impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_owned())
    }
}
impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}
impl From<Vec<Json>> for Json {
    fn from(v: Vec<Json>) -> Json {
        Json::Arr(v)
    }
}

/// Deepest nesting [`parse`] accepts, so hostile input cannot exhaust the stack.
pub const MAX_DEPTH: usize = 64;

/// Objects with more keys than this look for a repeated key in a map instead of scanning, so a
/// large object (a body may be 100 MiB) parses in linear time (SEC-04).
const SCAN_KEYS: usize = 16;

/// Parses one JSON text (surrounding whitespace allowed).
pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = Parser { b: s.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(format!("trailing data at {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at {}", self.i))
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.b[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return self.err("too deep");
        }
        self.ws();
        let Some(&c) = self.b.get(self.i) else { return self.err("unexpected end") };
        match c {
            b'{' => {
                self.i += 1;
                let mut m: Vec<(String, Json)> = Vec::new();
                // where each key is, once the object is too big to scan for a repeated one
                let mut index: HashMap<String, usize> = HashMap::new();
                self.ws();
                if self.eat("}") {
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        return self.err("expected a key");
                    }
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return self.err("expected ':'");
                    }
                    let v = self.value(depth + 1)?;
                    // a repeated key keeps its place and takes the last value
                    if m.len() < SCAN_KEYS {
                        match m.iter().position(|(key, _)| *key == k) {
                            Some(i) => m[i].1 = v,
                            None => m.push((k, v)),
                        }
                        if m.len() == SCAN_KEYS {
                            index = m.iter().enumerate().map(|(i, (key, _))| (key.clone(), i)).collect();
                        }
                    } else {
                        match index.entry(k) {
                            Entry::Occupied(e) => m[*e.get()].1 = v,
                            Entry::Vacant(e) => {
                                let k = e.key().clone();
                                e.insert(m.len());
                                m.push((k, v));
                            }
                        }
                    }
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(Json::Obj(m));
                    }
                    return self.err("expected ',' or '}'");
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Ok(Json::Arr(v));
                }
                loop {
                    v.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Json::Arr(v));
                    }
                    return self.err("expected ',' or ']'");
                }
            }
            b'"' => Ok(Json::Str(self.string()?)),
            b't' if self.eat("true") => Ok(Json::Bool(true)),
            b'f' if self.eat("false") => Ok(Json::Bool(false)),
            b'n' if self.eat("null") => Ok(Json::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => self.err("unexpected character"),
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        let digits = |p: &mut Parser<'_>| {
            let s = p.i;
            while p.i < p.b.len() && p.b[p.i].is_ascii_digit() {
                p.i += 1;
            }
            p.i - s
        };
        let int_start = self.i;
        let n = digits(self);
        if n == 0 || (n > 1 && self.b[int_start] == b'0') {
            return self.err("bad number");
        }
        let mut float = false;
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            float = true;
            if digits(self) == 0 {
                return self.err("bad fraction");
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            float = true;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return self.err("bad exponent");
            }
        }
        // Only ASCII was consumed, so this slice is valid UTF-8.
        let text = std::str::from_utf8(&self.b[start..self.i]).map_err(|e| e.to_string())?;
        if !float {
            if let Ok(i) = text.parse::<i64>() {
                return Ok(Json::Int(i));
            }
        }
        // JSON has no infinities, so a number too large for a double is refused (RFC 8259
        // section 6 leaves the range to the implementation).
        match text.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(Json::Float(f)),
            _ => self.err("number out of range"),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let Some(h) = self.b.get(self.i..self.i + 4) else { return self.err("short escape") };
        let h = std::str::from_utf8(h).map_err(|e| e.to_string())?;
        let v = u32::from_str_radix(h, 16).map_err(|_| format!("bad escape at {}", self.i))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // the opening quote
        let mut out = Vec::new();
        loop {
            let Some(&c) = self.b.get(self.i) else { return self.err("unterminated string") };
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|_| "invalid UTF-8".to_owned()),
                b'\\' => {
                    let Some(&e) = self.b.get(self.i) else { return self.err("unterminated escape") };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xd800..0xdc00).contains(&hi) {
                                if !self.eat("\\u") {
                                    return self.err("lone surrogate");
                                }
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return self.err("bad surrogate pair");
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            match char::from_u32(cp) {
                                Some(ch) => ch,
                                None => return self.err("bad code point"),
                            }
                        }
                        _ => return self.err("bad escape"),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1f => return self.err("control character in string"),
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = Json::obj()
            .with("a", 1i64)
            .with("b", "x\"y\\\n<")
            .with("c", Json::Arr(vec![Json::Null, true.into(), 1.5f64.into(), 2.0f64.into()]))
            .with("d", Json::obj());
        let s = v.to_json();
        assert_eq!(s, r#"{"a":1,"b":"x\"y\\\n\u003c","c":[null,true,1.5,2.0],"d":{}}"#);
        assert_eq!(parse(&s).unwrap(), v);
    }

    #[test]
    fn parses_numbers_strings_and_rejects_junk() {
        assert_eq!(parse(" -12 ").unwrap(), Json::Int(-12));
        assert_eq!(parse("1e3").unwrap(), Json::Float(1000.0));
        assert_eq!(parse("\"\\u00e9\\ud83d\\ude00\"").unwrap(), Json::Str("é😀".into()));
        for bad in [
            "",
            "{",
            "[1,]",
            "01",
            "1.",
            "\"\\ud800\"",
            "{\"a\" 1}",
            "tru",
            "1 2",
            "\"\u{1}\"",
            "1e999",
            "-1e400",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = "[".repeat(MAX_DEPTH + 2) + &"]".repeat(MAX_DEPTH + 2);
        assert!(parse(&deep).is_err());
        assert_eq!(parse(r#"{"a":1,"a":2}"#).unwrap(), Json::obj().with("a", 2i64));
    }

    /// SEC-04: a repeated key is found through a map in a big object, so one with many keys
    /// parses in linear time; a repeated key still keeps its first place and its last value.
    #[test]
    fn a_big_object_parses_in_linear_time() {
        let n = 60_000;
        let mut text = String::from("{");
        for i in 0..n {
            write!(text, "\"key{i}\":{i},").unwrap();
        }
        text.push_str("\"key3\":-3,\"key40\":-40}");
        let start = std::time::Instant::now();
        let v = parse(&text).unwrap();
        let took = start.elapsed();
        let Json::Obj(m) = &v else { panic!("not an object") };
        assert_eq!(m.len(), n);
        assert_eq!(m[3], ("key3".to_owned(), Json::Int(-3)));
        assert_eq!(m[40], ("key40".to_owned(), Json::Int(-40)));
        assert_eq!(m[n - 1].0, format!("key{}", n - 1));
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn accessors() {
        let v = parse(r#"{"b":1,"s":"true","f":3.0,"i":-1}"#).unwrap();
        assert_eq!(v.get("b").and_then(Json::as_bool), Some(true));
        assert_eq!(v.get("s").and_then(Json::as_bool), Some(true));
        assert_eq!(v.get("f").and_then(Json::as_i64), Some(3));
        assert_eq!(v.get("i").and_then(Json::as_i64), Some(-1));
        assert_eq!(Json::Float(f64::INFINITY).to_json(), "null");
        // found by the fuzzer: integral floats stay floats through a round trip
        for f in [7.766666666766667e18, 1e300, -2.0, 0.1] {
            assert_eq!(parse(&Json::Float(f).to_json()).unwrap(), Json::Float(f));
        }
    }
}
