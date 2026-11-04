//! A small JSON value (RFC 8259) for the search helper: GitHub's release listing, Jackett's
//! indexer listing and configuration items, and its `ServerConfig.json`, which is read, changed
//! in a few fields and written back with every other field kept. Objects keep their order.

use std::fmt::Write as _;

/// Deepest nesting accepted, so hostile input cannot exhaust the stack.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Kept as written, so a 64-bit id or a float goes back unchanged.
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Sets `key` in an object (in place when it is there, else at the end); a non-object
    /// becomes an empty object first.
    pub fn set(&mut self, key: &str, v: Json) {
        if !matches!(self, Json::Object(_)) {
            *self = Json::Object(Vec::new());
        }
        let Json::Object(o) = self else { unreachable!() };
        match o.iter_mut().find(|(k, _)| k == key) {
            Some((_, old)) => *old = v,
            None => o.push((key.to_owned(), v)),
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Json::as_str)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Number(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Array(a) => a,
            _ => &[],
        }
    }

    pub fn num(n: impl std::fmt::Display) -> Json {
        Json::Number(n.to_string())
    }

    pub fn string(s: impl Into<String>) -> Json {
        Json::String(s.into())
    }

    /// Compact JSON text.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, None, 0);
        out
    }

    /// Indented JSON text (two spaces), as Newtonsoft's `Formatting.Indented` writes it.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Some(2), 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        let newline = |out: &mut String, d: usize| {
            if let Some(i) = indent {
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', i * d));
            }
        };
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => out.push_str(n),
            Json::String(s) => write_str(s, out),
            Json::Array(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    v.write(out, indent, depth + 1);
                }
                if !a.is_empty() {
                    newline(out, depth);
                }
                out.push(']');
            }
            Json::Object(o) => {
                out.push('{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    write_str(k, out);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, depth + 1);
                }
                if !o.is_empty() {
                    newline(out, depth);
                }
                out.push('}');
            }
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parses one JSON text (surrounding whitespace and a UTF-8 byte order mark allowed).
pub fn parse(text: &str) -> Result<Json, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("JSON: unexpected data at {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|b| b" \t\r\n".contains(b)) {
            self.i += 1;
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("JSON: {what} at {}", self.i))
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return self.err("nested too deeply");
        }
        self.ws();
        match self.s.get(self.i) {
            None => self.err("unexpected end"),
            Some(b'{') => {
                self.i += 1;
                let mut o = Vec::new();
                self.ws();
                if self.eat("}") {
                    return Ok(Json::Object(o));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return self.err("expected a key");
                    }
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return self.err("expected ':'");
                    }
                    let v = self.value(depth + 1)?;
                    o.push((k, v));
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(Json::Object(o));
                    }
                    return self.err("expected ',' or '}'");
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Ok(Json::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Json::Array(a));
                    }
                    return self.err("expected ',' or ']'");
                }
            }
            Some(b'"') => self.string().map(Json::String),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while self.s.get(self.i).is_some_and(|b| b"+-.eE0123456789".contains(b)) {
                    self.i += 1;
                }
                let n = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
                if n.parse::<f64>().is_err() {
                    return self.err("bad number");
                }
                Ok(Json::Number(n.to_owned()))
            }
            Some(_) => self.err("unexpected character"),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok());
        let Some(v) = h.and_then(|h| u32::from_str_radix(h, 16).ok()) else {
            return self.err("bad \\u escape");
        };
        self.i += 4;
        Ok(v)
    }

    /// At the opening quote.
    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(&b) = self.s.get(self.i) else { return self.err("unterminated string") };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { return self.err("unterminated string") };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xd800..0xdc00).contains(&cp) && self.eat("\\u") {
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                                }
                            }
                            char::from_u32(cp).unwrap_or('\u{fffd}')
                        }
                        _ => return self.err("bad escape"),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                b => out.push(b),
            }
        }
        String::from_utf8(out).or_else(|_| self.err("not UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_writes() {
        let v =
            parse("\u{feff} {\"a\": [1, -2.5e3, true, false, null], \"b\": \"x\\\"\\u00e9\\ud83d\\ude00\"} ")
                .unwrap();
        assert_eq!(v.get("a").unwrap().as_array().len(), 5);
        assert_eq!(v.get("a").unwrap().as_array()[0].as_u64(), Some(1));
        assert_eq!(v.str("b"), Some("x\"é😀"));
        assert_eq!(v.to_text(), "{\"a\":[1,-2.5e3,true,false,null],\"b\":\"x\\\"é😀\"}");
        let mut o = parse("{\"Port\": 9117, \"Other\": \"kept\"}").unwrap();
        o.set("Port", Json::num(5000));
        o.set("New", Json::Bool(false));
        assert_eq!(o.to_pretty(), "{\n  \"Port\": 5000,\n  \"Other\": \"kept\",\n  \"New\": false\n}");
        assert_eq!(parse(&o.to_pretty()).unwrap(), o);
        for bad in ["", "{", "[1,]", "{\"a\" 1}", "tru", "\"x", "1 2", "{\"a\":1,}"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&"[".repeat(100)).is_err());
    }
}
