// Ported from libtorrent-rasterbar 2.0.11: src/entry.cpp, include/libtorrent/entry.hpp,
// include/libtorrent/bencode.hpp (encoder)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: a Rust enum instead of a tagged union; accessors that would throw
// `invalid_entry_type` panic instead (they are only used on values we build ourselves).

//! Owned bencode values, for building messages, resume data and `.torrent` files.

use std::collections::BTreeMap;

use super::bdecode::{Node, NodeType};

/// An owned bencoded value (libtorrent `entry`). Dictionaries are sorted by key, so encoding is
/// always canonical.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Entry {
    /// Encodes as an empty string (`0:`), like libtorrent's `undefined_t`.
    #[default]
    Undefined,
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Entry>),
    Dict(BTreeMap<Vec<u8>, Entry>),
    /// Already-bencoded bytes, copied verbatim into the output.
    Preformatted(Vec<u8>),
}

impl Entry {
    pub fn new_dict() -> Entry {
        Entry::Dict(BTreeMap::new())
    }

    pub fn new_list() -> Entry {
        Entry::List(Vec::new())
    }

    /// Deep copy of a decoded value (libtorrent `entry::operator=(bdecode_node const&)`).
    pub fn from_node(n: Node<'_>) -> Entry {
        match n.node_type() {
            NodeType::Str => Entry::Bytes(n.string_value().to_vec()),
            NodeType::Int => Entry::Int(n.int_value()),
            NodeType::List => Entry::List(n.list_iter().map(Entry::from_node).collect()),
            NodeType::Dict => {
                Entry::Dict(n.dict_iter().map(|(k, v)| (k.to_vec(), Entry::from_node(v))).collect())
            }
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Entry::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Entry::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    pub fn as_list(&self) -> Option<&[Entry]> {
        match self {
            Entry::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Entry>> {
        match self {
            Entry::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// The list, turning an undefined entry into an empty list first.
    ///
    /// # Panics
    /// If the entry is something other than a list or undefined.
    pub fn list_mut(&mut self) -> &mut Vec<Entry> {
        if matches!(self, Entry::Undefined) {
            *self = Entry::new_list();
        }
        match self {
            Entry::List(l) => l,
            other => panic!("entry is not a list: {other:?}"),
        }
    }

    /// The dictionary, turning an undefined entry into an empty dict first.
    ///
    /// # Panics
    /// If the entry is something other than a dict or undefined.
    pub fn dict_mut(&mut self) -> &mut BTreeMap<Vec<u8>, Entry> {
        if matches!(self, Entry::Undefined) {
            *self = Entry::new_dict();
        }
        match self {
            Entry::Dict(d) => d,
            other => panic!("entry is not a dictionary: {other:?}"),
        }
    }

    /// `entry[key]`: the value under `key`, inserted as undefined if missing.
    pub fn key(&mut self, key: impl AsRef<[u8]>) -> &mut Entry {
        self.dict_mut().entry(key.as_ref().to_vec()).or_default()
    }

    /// libtorrent `find_key`.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Option<&Entry> {
        self.as_dict()?.get(key.as_ref())
    }

    pub fn get_mut(&mut self, key: impl AsRef<[u8]>) -> Option<&mut Entry> {
        match self {
            Entry::Dict(d) => d.get_mut(key.as_ref()),
            _ => None,
        }
    }

    /// Bencodes into a new buffer.
    pub fn bencode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.bencode_into(&mut out);
        out
    }

    /// Bencodes onto the end of `out`.
    pub fn bencode_into(&self, out: &mut Vec<u8>) {
        let mut buf = [0u8; 21];
        match self {
            Entry::Int(v) => {
                out.push(b'i');
                out.extend_from_slice(integer_to_str(&mut buf, *v).as_bytes());
                out.push(b'e');
            }
            Entry::Bytes(s) => write_string(out, s),
            Entry::List(l) => {
                out.push(b'l');
                for item in l {
                    item.bencode_into(out);
                }
                out.push(b'e');
            }
            Entry::Dict(d) => {
                out.push(b'd');
                for (k, v) in d {
                    write_string(out, k);
                    v.bencode_into(out);
                }
                out.push(b'e');
            }
            Entry::Preformatted(p) => out.extend_from_slice(p),
            Entry::Undefined => out.extend_from_slice(b"0:"),
        }
    }

    /// Human-readable rendering (libtorrent `entry::to_string`).
    pub fn to_string_pretty(&self, single_line: bool) -> String {
        let mut out = String::new();
        to_string_impl(&mut out, self, 0, single_line);
        out
    }
}

fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    let mut buf = [0u8; 21];
    out.extend_from_slice(integer_to_str(&mut buf, s.len() as i64).as_bytes());
    out.push(b':');
    out.extend_from_slice(s);
}

/// Decimal formatting into a stack buffer (libtorrent `aux::integer_to_str`), handles `i64::MIN`.
pub fn integer_to_str(buf: &mut [u8; 21], val: i64) -> &str {
    let mut pos = buf.len();
    // work in the negative range, which is larger, so i64::MIN fits
    let negative = val < 0;
    let mut v = if negative { val } else { -val };
    loop {
        pos -= 1;
        buf[pos] = b'0' + (-(v % 10)) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if negative {
        pos -= 1;
        buf[pos] = b'-';
    }
    // only ASCII digits and '-' were written
    std::str::from_utf8(&buf[pos..]).expect("ascii")
}

fn print_string(s: &[u8]) -> String {
    if s.iter().any(|&c| !(32..127).contains(&c)) {
        crate::util::hex::encode(s)
    } else {
        String::from_utf8_lossy(s).into_owned()
    }
}

fn add_indent(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

fn to_string_impl(out: &mut String, e: &Entry, indent: usize, single_line: bool) {
    match e {
        Entry::Int(v) => out.push_str(&v.to_string()),
        Entry::Bytes(s) => {
            out.push('\'');
            out.push_str(&print_string(s));
            out.push('\'');
        }
        Entry::List(l) => {
            let indent = indent + 1;
            out.push_str(if single_line { "[ " } else { "[\n" });
            for (i, item) in l.iter().enumerate() {
                if i > 0 {
                    out.push_str(if single_line { ", " } else { ",\n" });
                }
                if !single_line {
                    add_indent(out, indent);
                }
                to_string_impl(out, item, indent, single_line);
            }
            out.push_str(" ]");
        }
        Entry::Dict(d) => {
            let indent = indent + 1;
            out.push_str(if single_line { "{ " } else { "{\n" });
            for (i, (k, v)) in d.iter().enumerate() {
                if i > 0 {
                    out.push_str(if single_line { ", " } else { ",\n" });
                }
                if !single_line {
                    add_indent(out, indent);
                }
                out.push('\'');
                out.push_str(&print_string(k));
                out.push_str("': ");
                to_string_impl(out, v, indent + 1, single_line);
            }
            out.push_str(" }");
        }
        Entry::Preformatted(_) => out.push_str("<preformatted>"),
        Entry::Undefined => out.push_str("<uninitialized>"),
    }
}

impl From<i64> for Entry {
    fn from(v: i64) -> Entry {
        Entry::Int(v)
    }
}

impl From<i32> for Entry {
    fn from(v: i32) -> Entry {
        Entry::Int(v as i64)
    }
}

impl From<u32> for Entry {
    fn from(v: u32) -> Entry {
        Entry::Int(v as i64)
    }
}

impl From<&str> for Entry {
    fn from(v: &str) -> Entry {
        Entry::Bytes(v.as_bytes().to_vec())
    }
}

impl From<String> for Entry {
    fn from(v: String) -> Entry {
        Entry::Bytes(v.into_bytes())
    }
}

impl From<&[u8]> for Entry {
    fn from(v: &[u8]) -> Entry {
        Entry::Bytes(v.to_vec())
    }
}

impl From<Vec<u8>> for Entry {
    fn from(v: Vec<u8>) -> Entry {
        Entry::Bytes(v)
    }
}

impl From<Vec<Entry>> for Entry {
    fn from(v: Vec<Entry>) -> Entry {
        Entry::List(v)
    }
}

impl From<BTreeMap<Vec<u8>, Entry>> for Entry {
    fn from(v: BTreeMap<Vec<u8>, Entry>) -> Entry {
        Entry::Dict(v)
    }
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_bencoding.cpp and the entry tests in
    //! test/test_bdecode.cpp.
    use super::*;
    use crate::bencode::bdecode;

    fn roundtrip(e: &Entry) -> Entry {
        let buf = e.bencode();
        Entry::from_node(bdecode(&buf[..]).unwrap().root().unwrap())
    }

    #[test]
    fn construct() {
        assert_eq!(Entry::from("abc123").as_bytes(), Some(&b"abc123"[..]));
        assert_eq!(Entry::from(String::from("abc123")).as_str(), Some("abc123"));
        assert_eq!(Entry::from(4).as_int(), Some(4));
    }

    #[test]
    fn strings() {
        let e = Entry::from("spam");
        assert_eq!(e.bencode(), b"4:spam");
        assert_eq!(roundtrip(&e), e);
    }

    #[test]
    fn integers() {
        for (v, enc) in [(3, &b"i3e"[..]), (-3, b"i-3e"), (0, b"i0e")] {
            let e = Entry::from(v);
            assert_eq!(e.bencode(), enc);
            assert_eq!(roundtrip(&e), e);
        }
    }

    #[test]
    fn lists() {
        let e = Entry::from(vec![Entry::from("spam"), Entry::from("eggs")]);
        assert_eq!(e.bencode(), b"l4:spam4:eggse");
        assert_eq!(roundtrip(&e), e);
    }

    #[test]
    fn dictionaries() {
        let mut e = Entry::new_dict();
        *e.key("spam") = "eggs".into();
        *e.key("cow") = "moo".into();
        assert_eq!(e.bencode(), b"d3:cow3:moo4:spam4:eggse");
        assert_eq!(roundtrip(&e), e);
    }

    #[test]
    fn preformatted() {
        assert_eq!(Entry::Preformatted(b"foobar".to_vec()).bencode(), b"foobar");
        let mut e = Entry::new_dict();
        *e.key("info") = Entry::Preformatted(b"foobar".to_vec());
        assert_eq!(e.bencode(), b"d4:infofoobare");
    }

    #[test]
    fn undefined() {
        assert_eq!(Entry::Undefined.bencode(), b"0:");
        let mut e = Entry::new_dict();
        *e.key("info") = Entry::Undefined;
        assert_eq!(e.bencode(), b"d4:info0:e");
    }

    #[test]
    fn implicit_construct() {
        let mut e = Entry::new_list();
        e.list_mut().push(Entry::new_list());
        assert!(matches!(e.list_mut().last(), Some(Entry::List(_))));
        let mut u = Entry::Undefined;
        u.list_mut().push(1.into());
        assert_eq!(u.bencode(), b"li1ee");
    }

    #[test]
    fn print_dict() {
        let mut e = Entry::Undefined;
        *e.key("foo") = "bar".into();
        *e.key("bar") = "foo".into();
        assert_eq!(e.to_string_pretty(true), "{ 'bar': 'foo', 'foo': 'bar' }");
        assert_eq!(e.to_string_pretty(false), "{\n 'bar': 'foo',\n 'foo': 'bar' }");
    }

    #[test]
    fn print_list() {
        let mut e = Entry::Undefined;
        e.list_mut().push("foo".into());
        e.list_mut().push("bar".into());
        assert_eq!(e.to_string_pretty(true), "[ 'foo', 'bar' ]");
        assert_eq!(e.to_string_pretty(false), "[\n 'foo',\n 'bar' ]");
    }

    #[test]
    fn print_scalars() {
        assert_eq!(Entry::from(1337).to_string_pretty(true), "1337");
        assert_eq!(Entry::from(1337).to_string_pretty(false), "1337");
        assert_eq!(Entry::from("foobar").to_string_pretty(true), "'foobar'");
        assert_eq!(Entry::from("foobar").to_string_pretty(false), "'foobar'");
        assert_eq!(Entry::from(&b"\x01\xff"[..]).to_string_pretty(true), "'01ff'");
    }

    #[test]
    fn print_deep_dict() {
        let mut e = Entry::Undefined;
        e.key("strings").list_mut().push("foo".into());
        e.key("strings").list_mut().push("bar".into());
        for i in 1..=3 {
            e.key("ints").list_mut().push(i.into());
        }
        *e.key("a") = "foobar".into();
        assert_eq!(
            e.to_string_pretty(true),
            "{ 'a': 'foobar', 'ints': [ 1, 2, 3 ], 'strings': [ 'foo', 'bar' ] }"
        );
        assert_eq!(
            e.to_string_pretty(false),
            "{\n 'a': 'foobar',\n 'ints': [\n   1,\n   2,\n   3 ],\n 'strings': [\n   'foo',\n   'bar' ] }"
        );
    }

    #[test]
    fn dict_constructor() {
        let d: BTreeMap<Vec<u8>, Entry> =
            [(b"foo".to_vec(), Entry::from("bar")), (b"bar".to_vec(), Entry::from(1234))]
                .into_iter()
                .collect();
        assert_eq!(Entry::from(d).to_string_pretty(false), "{\n 'bar': 1234,\n 'foo': 'bar' }");
    }

    #[test]
    fn integer_to_str_values() {
        let mut buf = [0u8; 21];
        for v in [0i64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 123, 1234, 12345, 123456, 123456789012345678] {
            assert_eq!(integer_to_str(&mut buf, v), v.to_string());
            assert_eq!(integer_to_str(&mut buf, -v), (-v).to_string());
        }
        assert_eq!(integer_to_str(&mut buf, i64::MAX), "9223372036854775807");
        assert_eq!(integer_to_str(&mut buf, i64::MIN), "-9223372036854775808");
    }

    #[test]
    fn get_and_get_mut() {
        let mut e = Entry::new_dict();
        *e.key("a") = 1.into();
        assert_eq!(e.get("a"), Some(&Entry::Int(1)));
        assert_eq!(e.get("b"), None);
        *e.get_mut("a").unwrap() = 2.into();
        assert_eq!(e.get("a").and_then(Entry::as_int), Some(2));
        assert_eq!(Entry::from(1).get("a"), None);
    }
}
