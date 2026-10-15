// Ported from libtorrent-rasterbar 2.0.11: src/bdecode.cpp, include/libtorrent/bdecode.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: owner (`Bdecoded<B>`) and handle (`Node`) are separate types; absent nodes are `None`;
// iteration replaces the mutable `list_at` / `dict_at` lookup cache; containers closed while
// unwinding after an error link past their end token like normally closed ones (libtorrent links
// them to their own end token, which breaks `has_soft_error` on partial trees).

//! Zero-copy bencode decoder.
//!
//! Decoding produces a flat array of 8-byte tokens pointing into the input buffer, exactly like
//! libtorrent's `bdecode_node`: no allocation per value, strings are slices of the input, and the
//! token array can be kept next to the buffer (as `torrent_info` does with the info dictionary).
//! On failure, the tokens parsed so far are still terminated into a valid tree
//! ([`bdecode_partial`]), so callers can inspect what came before the error.

use std::fmt;

/// libtorrent's default nesting depth limit.
pub const DEFAULT_DEPTH_LIMIT: usize = 100;
/// libtorrent's default token limit (an upper bound on total values).
pub const DEFAULT_TOKEN_LIMIT: usize = 2_000_000;

const MAX_OFFSET: usize = (1 << 29) - 1;
const MAX_NEXT_ITEM: usize = (1 << 29) - 1;
/// A string token stores its header (length digits plus ':') in 3 bits, offset by 2 for short
/// strings and by 10 for long ones.
const SHORT_STRING_MAX_HEADER: usize = (1 << 3) - 1 + 2;
const LONG_STRING_MAX_HEADER: usize = 8 + (1 << 3) - 1 + 2;

/// libtorrent `bdecode_errors::error_code_enum`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BdecodeErrorKind {
    ExpectedDigit,
    ExpectedColon,
    UnexpectedEof,
    ExpectedValue,
    DepthExceeded,
    LimitExceeded,
    Overflow,
}

impl BdecodeErrorKind {
    /// libtorrent's message text for the `bdecode` error category.
    pub fn message(self) -> &'static str {
        match self {
            BdecodeErrorKind::ExpectedDigit => "expected digit in bencoded string",
            BdecodeErrorKind::ExpectedColon => "expected colon in bencoded string",
            BdecodeErrorKind::UnexpectedEof => "unexpected end of file in bencoded string",
            BdecodeErrorKind::ExpectedValue => {
                "expected value (list, dict, int or string) in bencoded string"
            }
            BdecodeErrorKind::DepthExceeded => "bencoded nesting depth exceeded",
            BdecodeErrorKind::LimitExceeded => "bencoded item count limit exceeded",
            BdecodeErrorKind::Overflow => "integer overflow",
        }
    }
}

/// A decode failure and the byte offset where it was detected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BdecodeError {
    pub kind: BdecodeErrorKind,
    pub pos: usize,
}

impl fmt::Display for BdecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.kind.message(), self.pos)
    }
}

impl std::error::Error for BdecodeError {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum TokenType {
    Dict = 1,
    List = 2,
    Str = 3,
    Int = 4,
    LongStr = 5,
    End = 6,
}

/// libtorrent `aux::bdecode_token`: offset:29 + type:3, next_item:29 + header:3.
#[derive(Clone, Copy)]
struct Token {
    a: u32,
    b: u32,
}

impl Token {
    /// `next_item` is the relative index of the next sibling; `header_size` is the string
    /// header length (digits plus ':'), only for strings.
    fn new(offset: usize, ty: TokenType, next_item: usize, header_size: usize) -> Token {
        debug_assert!(offset <= MAX_OFFSET && next_item <= MAX_NEXT_ITEM);
        let ty = if ty == TokenType::Str && header_size > SHORT_STRING_MAX_HEADER {
            TokenType::LongStr
        } else {
            ty
        };
        let header = match ty {
            TokenType::Str => header_size - 2,
            TokenType::LongStr => header_size - 8 - 2,
            _ => 0,
        };
        Token { a: offset as u32 | ((ty as u32) << 29), b: next_item as u32 | ((header as u32) << 29) }
    }

    fn offset(self) -> usize {
        (self.a & MAX_OFFSET as u32) as usize
    }

    fn ty(self) -> TokenType {
        match self.a >> 29 {
            1 => TokenType::Dict,
            2 => TokenType::List,
            3 => TokenType::Str,
            4 => TokenType::Int,
            5 => TokenType::LongStr,
            _ => TokenType::End,
        }
    }

    fn next_item(self) -> usize {
        (self.b & MAX_NEXT_ITEM as u32) as usize
    }

    fn set_next_item(&mut self, n: usize) {
        self.b = (self.b & !(MAX_NEXT_ITEM as u32)) | n as u32;
    }

    fn header(self) -> usize {
        (self.b >> 29) as usize
    }

    fn is_string(self) -> bool {
        matches!(self.ty(), TokenType::Str | TokenType::LongStr)
    }

    /// Offset of the string payload relative to the token's offset.
    fn start_offset(self) -> usize {
        if self.ty() == TokenType::Str { self.header() + 2 } else { self.header() + 8 + 2 }
    }
}

/// The result of decoding: the buffer (borrowed or owned) and its token array.
///
/// `B` is `&[u8]` for zero-copy decoding of a caller-owned buffer, or an owning type (`Vec<u8>`,
/// `Box<[u8]>`, `Arc<[u8]>`) to keep the buffer alive alongside the tokens.
#[derive(Clone)]
pub struct Bdecoded<B> {
    buf: B,
    tokens: Vec<Token>,
    /// Bytes consumed by the parse (libtorrent `m_buffer_size`).
    consumed: usize,
}

impl<B: AsRef<[u8]>> Bdecoded<B> {
    /// The top-level value, or `None` if nothing could be parsed.
    pub fn root(&self) -> Option<Node<'_>> {
        match self.tokens.first() {
            Some(t) if t.ty() != TokenType::End => {
                Some(Node { buf: self.buf.as_ref(), tokens: &self.tokens, idx: 0 })
            }
            _ => None,
        }
    }

    /// Bytes of input that make up the decoded value.
    pub fn consumed(&self) -> usize {
        self.consumed
    }

    pub fn buffer(&self) -> &B {
        &self.buf
    }

    /// Tokens allocated, for memory accounting.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Moves the tree onto an identical copy of its buffer (libtorrent
    /// `switch_underlying_buffer`), e.g. from a borrowed slice to an owned `Arc<[u8]>`.
    pub fn rebind<C: AsRef<[u8]>>(self, buf: C) -> Bdecoded<C> {
        assert!(buf.as_ref().len() >= self.consumed, "rebind target is shorter than the decoded data");
        debug_assert_eq!(&buf.as_ref()[..self.consumed], &self.buf.as_ref()[..self.consumed]);
        Bdecoded { buf, tokens: self.tokens, consumed: self.consumed }
    }
}

impl<B: AsRef<[u8]>> fmt::Debug for Bdecoded<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.root() {
            Some(n) => f.write_str(&print_entry(n, true, 0)),
            None => f.write_str("none"),
        }
    }
}

/// libtorrent `bdecode_node::type_t` (without `none_t`, which is `Option::None` here).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeType {
    Dict,
    List,
    Str,
    Int,
}

/// A cheap handle to one value inside a [`Bdecoded`] tree.
///
/// Accessors are lenient: calling a dict method on a list (or any other type mismatch) returns
/// `None`, an empty iterator or 0 instead of panicking, because the data is untrusted.
#[derive(Clone, Copy)]
pub struct Node<'t> {
    buf: &'t [u8],
    tokens: &'t [Token],
    idx: usize,
}

impl<'t> Node<'t> {
    fn token(&self) -> Token {
        self.tokens[self.idx]
    }

    fn at(&self, idx: usize) -> Node<'t> {
        Node { buf: self.buf, tokens: self.tokens, idx }
    }

    pub fn node_type(&self) -> NodeType {
        match self.token().ty() {
            TokenType::Dict => NodeType::Dict,
            TokenType::List => NodeType::List,
            TokenType::Int => NodeType::Int,
            _ => NodeType::Str,
        }
    }

    pub fn is_dict(&self) -> bool {
        self.token().ty() == TokenType::Dict
    }

    pub fn is_list(&self) -> bool {
        self.token().ty() == TokenType::List
    }

    pub fn is_string(&self) -> bool {
        self.token().is_string()
    }

    pub fn is_int(&self) -> bool {
        self.token().ty() == TokenType::Int
    }

    /// The raw bencoded bytes of this value (libtorrent `data_section()`).
    pub fn data_section(&self) -> &'t [u8] {
        let t = self.token();
        let next = self.tokens[self.idx + t.next_item()];
        &self.buf[t.offset()..next.offset()]
    }

    /// Offset of this value in the decoded buffer.
    pub fn data_offset(&self) -> usize {
        self.token().offset()
    }

    // --- strings ---

    /// The bytes of a string value; empty for other types.
    pub fn string_value(&self) -> &'t [u8] {
        let t = self.token();
        if !t.is_string() {
            return &[];
        }
        let start = t.offset() + t.start_offset();
        let end = self.tokens[self.idx + 1].offset();
        &self.buf[start..end]
    }

    /// The string as UTF-8, if it is a string and valid UTF-8.
    pub fn str_value(&self) -> Option<&'t str> {
        if !self.is_string() {
            return None;
        }
        std::str::from_utf8(self.string_value()).ok()
    }

    pub fn string_length(&self) -> usize {
        self.string_value().len()
    }

    /// Offset of the string payload in the decoded buffer.
    pub fn string_offset(&self) -> usize {
        let t = self.token();
        t.offset() + t.start_offset()
    }

    // --- integers ---

    /// Integer value. Integers that overflow 64 bits read as 0, as in libtorrent (the parser only
    /// rejects values that are grossly too long). Non-integers read as 0.
    pub fn int_value(&self) -> i64 {
        self.try_int_value().unwrap_or(0)
    }

    /// Integer value, `None` for non-integers and overflow.
    pub fn try_int_value(&self) -> Option<i64> {
        let t = self.token();
        if t.ty() != TokenType::Int {
            return None;
        }
        // +1 skips the 'i'
        let body = &self.buf[t.offset() + 1..self.tokens[self.idx + 1].offset()];
        let negative = body.first() == Some(&b'-');
        let mut val = 0i64;
        let (_, err) = parse_int(&body[negative as usize..], b'e', &mut val);
        if err.is_some() {
            return None;
        }
        Some(if negative { -val } else { val })
    }

    // --- lists ---

    pub fn list_iter(&self) -> ListIter<'t> {
        let t = self.token();
        let next = if t.ty() == TokenType::List { self.idx + 1 } else { usize::MAX };
        ListIter { node: *self, next }
    }

    /// The `i`th list item. This walks from the start: prefer [`Node::list_iter`] in loops.
    pub fn list_at(&self, i: usize) -> Option<Node<'t>> {
        self.list_iter().nth(i)
    }

    pub fn list_size(&self) -> usize {
        self.list_iter().count()
    }

    pub fn list_string_value_at(&self, i: usize) -> Option<&'t [u8]> {
        self.list_at(i).filter(Node::is_string).map(|n| n.string_value())
    }

    pub fn list_int_value_at(&self, i: usize) -> Option<i64> {
        self.list_at(i).filter(Node::is_int).map(|n| n.int_value())
    }

    // --- dictionaries ---

    /// Key/value pairs in buffer order (sorted, for well-formed input).
    pub fn dict_iter(&self) -> DictIter<'t> {
        let t = self.token();
        let next = if t.ty() == TokenType::Dict { self.idx + 1 } else { usize::MAX };
        DictIter { node: *self, next }
    }

    pub fn dict_size(&self) -> usize {
        self.dict_iter().count()
    }

    /// The `i`th key and value. Walks from the start: prefer [`Node::dict_iter`] in loops.
    pub fn dict_at(&self, i: usize) -> Option<(&'t [u8], Node<'t>)> {
        self.dict_at_node(i).map(|(k, v)| (k.string_value(), v))
    }

    /// Like [`Node::dict_at`] but returns the key as a node (for its offset).
    pub fn dict_at_node(&self, i: usize) -> Option<(Node<'t>, Node<'t>)> {
        self.dict_iter().nodes().nth(i)
    }

    pub fn dict_find(&self, key: impl AsRef<[u8]>) -> Option<Node<'t>> {
        let key = key.as_ref();
        self.dict_iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    pub fn dict_find_dict(&self, key: impl AsRef<[u8]>) -> Option<Node<'t>> {
        self.dict_find(key).filter(Node::is_dict)
    }

    pub fn dict_find_list(&self, key: impl AsRef<[u8]>) -> Option<Node<'t>> {
        self.dict_find(key).filter(Node::is_list)
    }

    pub fn dict_find_string(&self, key: impl AsRef<[u8]>) -> Option<Node<'t>> {
        self.dict_find(key).filter(Node::is_string)
    }

    pub fn dict_find_int(&self, key: impl AsRef<[u8]>) -> Option<Node<'t>> {
        self.dict_find(key).filter(Node::is_int)
    }

    /// String value under `key`, if present and a string.
    pub fn dict_find_string_value(&self, key: impl AsRef<[u8]>) -> Option<&'t [u8]> {
        self.dict_find_string(key).map(|n| n.string_value())
    }

    /// UTF-8 string value under `key`, if present, a string, and valid UTF-8.
    pub fn dict_find_str(&self, key: impl AsRef<[u8]>) -> Option<&'t str> {
        self.dict_find_string(key).and_then(|n| n.str_value())
    }

    /// Integer value under `key`, if present and an integer.
    pub fn dict_find_int_value(&self, key: impl AsRef<[u8]>) -> Option<i64> {
        self.dict_find_int(key).map(|n| n.int_value())
    }

    /// Checks for things a strict encoder would never produce but the decoder accepts: leading
    /// zeros, unsorted and duplicate dictionary keys. Returns the first problem found.
    pub fn has_soft_error(&self) -> Option<&'static str> {
        let tokens = self.tokens;
        let buf = self.buf;
        let mut token = self.idx;
        let mut stack: Vec<usize> = Vec::with_capacity(DEFAULT_DEPTH_LIMIT);
        loop {
            let t = tokens[token];
            match t.ty() {
                TokenType::Int => {
                    if buf[t.offset() + 1] == b'0' && buf[t.offset() + 2] != b'e' {
                        return Some("leading zero in integer");
                    }
                }
                TokenType::Str | TokenType::LongStr => {
                    if buf[t.offset()] == b'0' && buf[t.offset() + 1] != b':' {
                        return Some("leading zero in string length");
                    }
                }
                TokenType::Dict | TokenType::List => stack.push(token),
                TokenType::End => {
                    let parent = stack.pop().expect("end token without an open container");
                    if tokens[parent].ty() == TokenType::Dict && token != parent + 1 {
                        // a non-empty dict ends here: check that the keys are sorted
                        let mut k1 = parent + 1;
                        loop {
                            let v1 = k1 + tokens[k1].next_item();
                            let k2 = v1 + tokens[v1].next_item();
                            if k2 == token {
                                break;
                            }
                            let v2 = k2 + tokens[k2].next_item();
                            let k1_start = tokens[k1].offset() + tokens[k1].start_offset();
                            let k2_start = tokens[k2].offset() + tokens[k2].start_offset();
                            let key1 = &buf[k1_start..tokens[v1].offset()];
                            let key2 = &buf[k2_start..tokens[v2].offset()];
                            match key1.cmp(key2) {
                                std::cmp::Ordering::Greater => return Some("unsorted dictionary key"),
                                std::cmp::Ordering::Equal => return Some("duplicate dictionary key"),
                                std::cmp::Ordering::Less => {}
                            }
                            k1 = k2;
                        }
                    }
                }
            }
            token += 1;
            if stack.is_empty() {
                return None;
            }
        }
    }
}

impl fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&print_entry(*self, true, 0))
    }
}

/// Iterates the items of a list node.
#[derive(Clone)]
pub struct ListIter<'t> {
    node: Node<'t>,
    next: usize,
}

impl<'t> Iterator for ListIter<'t> {
    type Item = Node<'t>;

    fn next(&mut self) -> Option<Node<'t>> {
        let t = *self.node.tokens.get(self.next)?;
        if t.ty() == TokenType::End {
            self.next = usize::MAX;
            return None;
        }
        let item = self.node.at(self.next);
        self.next += t.next_item();
        Some(item)
    }
}

/// Iterates the key/value pairs of a dict node.
#[derive(Clone)]
pub struct DictIter<'t> {
    node: Node<'t>,
    next: usize,
}

impl<'t> DictIter<'t> {
    /// Yields keys as nodes instead of byte slices.
    pub fn nodes(self) -> impl Iterator<Item = (Node<'t>, Node<'t>)> {
        let mut it = self;
        std::iter::from_fn(move || it.next_nodes())
    }

    fn next_nodes(&mut self) -> Option<(Node<'t>, Node<'t>)> {
        let kt = *self.node.tokens.get(self.next)?;
        if kt.ty() == TokenType::End {
            self.next = usize::MAX;
            return None;
        }
        let key = self.node.at(self.next);
        let value_idx = self.next + kt.next_item();
        let vt = self.node.tokens[value_idx];
        self.next = value_idx + vt.next_item();
        Some((key, self.node.at(value_idx)))
    }
}

impl<'t> Iterator for DictIter<'t> {
    type Item = (&'t [u8], Node<'t>);

    fn next(&mut self) -> Option<(&'t [u8], Node<'t>)> {
        self.next_nodes().map(|(k, v)| (k.string_value(), v))
    }
}

/// libtorrent `parse_int`: reads digits from the start of `data` up to `delimiter` (or the end),
/// accumulating into `val` (which must start at 0 or a positive value). Returns the index where
/// parsing stopped and the error, if any.
pub fn parse_int(data: &[u8], delimiter: u8, val: &mut i64) -> (usize, Option<BdecodeErrorKind>) {
    let mut i = 0;
    while i < data.len() && data[i] != delimiter {
        let c = data[i];
        if !c.is_ascii_digit() {
            return (i, Some(BdecodeErrorKind::ExpectedDigit));
        }
        if *val > i64::MAX / 10 {
            return (i, Some(BdecodeErrorKind::Overflow));
        }
        *val *= 10;
        let digit = (c - b'0') as i64;
        if *val > i64::MAX - digit {
            return (i, Some(BdecodeErrorKind::Overflow));
        }
        *val += digit;
        i += 1;
    }
    (i, None)
}

/// Finds the end of an integer body starting at `start` (just after 'i'). Detects gross overflow
/// only (more than 20 digits); exact overflow is checked when the value is read.
fn check_integer(data: &[u8], mut start: usize) -> (usize, Option<BdecodeErrorKind>) {
    let end = data.len();
    if start == end {
        return (start, Some(BdecodeErrorKind::UnexpectedEof));
    }
    if data[start] == b'-' {
        start += 1;
        if start == end {
            return (start, Some(BdecodeErrorKind::UnexpectedEof));
        }
    }
    let mut digits = 0;
    let mut err = None;
    loop {
        if !data[start].is_ascii_digit() {
            err = Some(BdecodeErrorKind::ExpectedDigit);
            break;
        }
        start += 1;
        digits += 1;
        if start == end {
            err = Some(BdecodeErrorKind::UnexpectedEof);
            break;
        }
        if data[start] == b'e' {
            break;
        }
    }
    if digits > 20 {
        err = Some(BdecodeErrorKind::Overflow);
    }
    (start, err)
}

/// Decodes with libtorrent's default limits.
pub fn bdecode<B: AsRef<[u8]>>(buf: B) -> Result<Bdecoded<B>, BdecodeError> {
    bdecode_with_limits(buf, DEFAULT_DEPTH_LIMIT, DEFAULT_TOKEN_LIMIT)
}

pub fn bdecode_with_limits<B: AsRef<[u8]>>(
    buf: B,
    depth_limit: usize,
    token_limit: usize,
) -> Result<Bdecoded<B>, BdecodeError> {
    match bdecode_partial(buf, depth_limit, token_limit) {
        (tree, None) => Ok(tree),
        (_, Some(e)) => Err(e),
    }
}

/// Decodes as much as possible. On error, the returned tree holds everything parsed before the
/// failure, with open containers closed (a dict waiting for a value gets an empty dict).
pub fn bdecode_partial<B: AsRef<[u8]>>(
    buf: B,
    depth_limit: usize,
    token_limit: usize,
) -> (Bdecoded<B>, Option<BdecodeError>) {
    decode_impl(buf, depth_limit, token_limit, MAX_OFFSET)
}

#[derive(Clone, Copy)]
struct Frame {
    token: usize,
    /// For dicts: false while expecting a key, true while expecting a value.
    value_next: bool,
}

fn decode_impl<B: AsRef<[u8]>>(
    buf: B,
    depth_limit: usize,
    token_limit: usize,
    max_len: usize,
) -> (Bdecoded<B>, Option<BdecodeError>) {
    let data = buf.as_ref();
    if data.len() > max_len {
        let err = BdecodeError { kind: BdecodeErrorKind::LimitExceeded, pos: 0 };
        return (Bdecoded { buf, tokens: Vec::new(), consumed: 0 }, Some(err));
    }
    let end = data.len();
    let mut tokens: Vec<Token> = Vec::new();
    let mut stack: Vec<Frame> = Vec::with_capacity(depth_limit.min(1024));
    let mut tokens_left = token_limit;
    let mut start = 0usize;
    let mut error: Option<BdecodeError> = None;

    macro_rules! fail {
        ($kind:expr) => {{
            error = Some(BdecodeError { kind: $kind, pos: start });
            break;
        }};
    }

    loop {
        if start >= end {
            fail!(BdecodeErrorKind::UnexpectedEof);
        }
        if stack.len() >= depth_limit {
            fail!(BdecodeErrorKind::DepthExceeded);
        }
        if tokens_left == 0 {
            fail!(BdecodeErrorKind::LimitExceeded);
        }
        tokens_left -= 1;

        let t = data[start];
        let current_frame = stack.len();

        // inside a dict, keys must be strings (or the dict ends)
        if let Some(top) = stack.last() {
            if tokens[top.token].ty() == TokenType::Dict
                && !top.value_next
                && !t.is_ascii_digit()
                && t != b'e'
            {
                fail!(BdecodeErrorKind::ExpectedDigit);
            }
        }

        match t {
            b'd' | b'l' => {
                stack.push(Frame { token: tokens.len(), value_next: false });
                let ty = if t == b'd' { TokenType::Dict } else { TokenType::List };
                tokens.push(Token::new(start, ty, 0, 0));
                start += 1;
            }
            b'i' => {
                let int_start = start;
                let (pos, e) = check_integer(data, start + 1);
                if let Some(kind) = e {
                    // Report where the integer went wrong, but terminate the tree at its start.
                    error = Some(BdecodeError { kind, pos });
                    start = int_start;
                    break;
                }
                tokens.push(Token::new(int_start, TokenType::Int, 1, 1));
                debug_assert_eq!(data[pos], b'e');
                start = pos + 1;
            }
            b'e' => {
                let Some(&top) = stack.last() else { fail!(BdecodeErrorKind::UnexpectedEof) };
                if tokens[top.token].ty() == TokenType::Dict && top.value_next {
                    // a key without a value
                    fail!(BdecodeErrorKind::ExpectedValue);
                }
                // the container's next sibling comes right after its end token
                let distance = tokens.len() + 1 - top.token;
                if distance > MAX_NEXT_ITEM {
                    fail!(BdecodeErrorKind::LimitExceeded);
                }
                tokens.push(Token::new(start, TokenType::End, 1, 0));
                tokens[top.token].set_next_item(distance);
                stack.pop();
                start += 1;
            }
            _ => {
                // strings start with a digit
                if !t.is_ascii_digit() {
                    fail!(BdecodeErrorKind::ExpectedValue);
                }
                let mut len = (t - b'0') as i64;
                let str_start = start;
                start += 1;
                if start >= end {
                    fail!(BdecodeErrorKind::UnexpectedEof);
                }
                let (n, e) = parse_int(&data[start..], b':', &mut len);
                start += n;
                if let Some(kind) = e {
                    fail!(kind);
                }
                if start == end {
                    fail!(BdecodeErrorKind::ExpectedColon);
                }
                // remaining bytes after ':'
                let remaining = end - start - 1;
                if len as u64 > remaining as u64 {
                    fail!(BdecodeErrorKind::UnexpectedEof);
                }
                start += 1; // ':'
                let header = start - str_start;
                if header > LONG_STRING_MAX_HEADER {
                    fail!(BdecodeErrorKind::LimitExceeded);
                }
                tokens.push(Token::new(str_start, TokenType::Str, 1, header));
                start += len as usize;
            }
        }

        // a dict alternates between keys and values; flip it when a value starts in it
        if current_frame > 0 && stack.len() >= current_frame {
            let frame = &mut stack[current_frame - 1];
            if tokens[frame.token].ty() == TokenType::Dict {
                frame.value_next = !frame.value_next;
            }
        }

        if stack.is_empty() {
            break; // the top-level value is complete
        }
    }

    // After an error, close every open container so the partial tree is valid.
    while let Some(frame) = stack.pop() {
        debug_assert!(error.is_some());
        if tokens[frame.token].ty() == TokenType::Dict && frame.value_next {
            // the dict was waiting for a value: give it an empty dict
            tokens.push(Token::new(start, TokenType::Dict, 2, 0));
            tokens.push(Token::new(start, TokenType::End, 0, 0));
        }
        tokens.push(Token::new(start, TokenType::End, 1, 0));
        let distance = tokens.len() - frame.token;
        tokens[frame.token].set_next_item(distance);
    }
    tokens.push(Token::new(start, TokenType::End, 0, 0));
    (Bdecoded { buf, tokens, consumed: start }, error)
}

/// Human-readable rendering of a decoded value (libtorrent `print_entry`). Short containers are
/// printed on one line; `single_line` forces that and abbreviates long strings.
pub fn print_entry(e: Node<'_>, single_line: bool, indent: usize) -> String {
    let mut sep = String::from(",\n");
    sep.extend(std::iter::repeat_n(' ', indent.min(197)));
    let newline_indent = &sep[1..];
    match e.node_type() {
        NodeType::Int => e.int_value().to_string(),
        NodeType::Str => {
            let mut ret = String::new();
            print_string(&mut ret, e.string_value(), single_line);
            ret
        }
        NodeType::List => {
            let mut ret = String::from("[");
            let one_liner = line_longer_than(e, 200).is_some() || single_line;
            if !one_liner {
                ret += newline_indent;
            }
            let n = e.list_size();
            for (i, item) in e.list_iter().enumerate() {
                if i == 0 && one_liner {
                    ret.push(' ');
                }
                ret += &print_entry(item, single_line, indent + 2);
                if i + 1 < n {
                    ret += if one_liner { ", " } else { &sep };
                } else {
                    ret += if one_liner { " " } else { newline_indent };
                }
            }
            ret.push(']');
            ret
        }
        NodeType::Dict => {
            let mut ret = String::from("{");
            let one_liner = line_longer_than(e, 200).is_some() || single_line;
            if !one_liner {
                ret += newline_indent;
            }
            let n = e.dict_size();
            for (i, (key, value)) in e.dict_iter().enumerate() {
                if i == 0 && one_liner {
                    ret.push(' ');
                }
                print_string(&mut ret, key, true);
                ret += ": ";
                ret += &print_entry(value, single_line, indent + 2);
                if i + 1 < n {
                    ret += if one_liner { ", " } else { &sep };
                } else {
                    ret += if one_liner { " " } else { newline_indent };
                }
            }
            ret.push('}');
            ret
        }
    }
}

/// Length of the single-line rendering, or `None` if it exceeds `limit`.
fn line_longer_than(e: Node<'_>, limit: i64) -> Option<i64> {
    let mut line_len: i64 = 0;
    match e.node_type() {
        NodeType::List => {
            line_len += 4;
            if line_len > limit {
                return None;
            }
            for item in e.list_iter() {
                line_len += line_longer_than(item, limit - line_len)? + 2;
            }
        }
        NodeType::Dict => {
            line_len += 4;
            if line_len > limit {
                return None;
            }
            for (key, value) in e.dict_iter() {
                line_len += 4 + key.len() as i64;
                if line_len > limit {
                    return None;
                }
                line_len += line_longer_than(value, limit - line_len)? + 1;
            }
        }
        NodeType::Str => line_len += 3 + e.string_length() as i64,
        NodeType::Int => {
            let mut val = e.int_value();
            while val > 0 {
                line_len += 1;
                val /= 10;
            }
            line_len += 2;
        }
    }
    (line_len <= limit).then_some(line_len)
}

fn print_string(ret: &mut String, s: &[u8], single_line: bool) {
    let printable = s.iter().all(|&c| (32..127).contains(&c));
    ret.push('\'');
    let len = s.len();
    if printable {
        // all bytes are ASCII here
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        if single_line && len > 30 {
            ret.push_str(&text(&s[..14]));
            ret.push_str("...");
            ret.push_str(&text(&s[len - 14..]));
        } else {
            ret.push_str(&text(s));
        }
    } else if single_line && len > 32 {
        escape_string(ret, &s[..25]);
        ret.push_str("...");
        escape_string(ret, &s[len - 4..]);
    } else {
        escape_string(ret, s);
    }
    ret.push('\'');
}

/// libtorrent `aux::escape_string`: hex if anything is non-printable, verbatim otherwise.
fn escape_string(ret: &mut String, s: &[u8]) {
    if s.iter().any(|&c| !(32..127).contains(&c)) {
        ret.push_str(&crate::util::hex::encode(s));
    } else {
        ret.push_str(&String::from_utf8_lossy(s));
    }
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_bdecode.cpp.
    use super::*;
    use BdecodeErrorKind::*;

    fn decode(b: &[u8]) -> (Bdecoded<&[u8]>, Option<BdecodeError>) {
        bdecode_partial(b, DEFAULT_DEPTH_LIMIT, DEFAULT_TOKEN_LIMIT)
    }

    fn ok(b: &[u8]) -> Bdecoded<&[u8]> {
        let (t, e) = decode(b);
        assert_eq!(e, None, "unexpected error decoding {:?}", String::from_utf8_lossy(b));
        t
    }

    fn err(b: &[u8]) -> BdecodeError {
        decode(b).1.expect("expected an error")
    }

    fn pe(b: &Bdecoded<&[u8]>) -> String {
        b.root().map_or_else(String::new, |n| print_entry(n, false, 0))
    }

    #[test]
    fn integer() {
        let b = b"i12453e";
        let t = ok(b);
        let e = t.root().unwrap();
        assert_eq!(e.data_section(), b);
        assert_eq!(e.node_type(), NodeType::Int);
        assert_eq!(e.int_value(), 12453);
    }

    #[test]
    fn string() {
        let b = b"26:abcdefghijklmnopqrstuvwxyz";
        let t = ok(b);
        let e = t.root().unwrap();
        assert_eq!(e.data_section(), b);
        assert_eq!(e.node_type(), NodeType::Str);
        assert_eq!(e.string_value(), b"abcdefghijklmnopqrstuvwxyz");
        assert_eq!(e.string_length(), 26);
    }

    #[test]
    fn string_prefix1() {
        // a string that is nearly too long for a short header
        let mut test = vec![0u8; 1_000_000 + 8];
        test[..8].copy_from_slice(b"1000000:");
        let t = ok(&test);
        let e = t.root().unwrap();
        assert_eq!(e.data_section(), &test[..]);
        assert_eq!(e.node_type(), NodeType::Str);
        assert_eq!(e.string_length(), 1_000_000);
        assert_eq!(e.string_offset(), 8);
    }

    #[test]
    fn list() {
        let b = b"li12453e3:aaae";
        let t = ok(b);
        let e = t.root().unwrap();
        assert_eq!(e.data_section(), b);
        assert_eq!(e.node_type(), NodeType::List);
        assert_eq!(e.list_size(), 2);
        assert_eq!(e.list_at(0).unwrap().node_type(), NodeType::Int);
        assert_eq!(e.list_at(1).unwrap().node_type(), NodeType::Str);
        assert_eq!(e.list_at(0).unwrap().int_value(), 12453);
        assert_eq!(e.list_at(1).unwrap().string_value(), b"aaa");
        assert_eq!(e.list_at(1).unwrap().string_length(), 3);
        assert_eq!(e.list_at(1).unwrap().data_section(), b"3:aaa");
    }

    #[test]
    fn dict() {
        let b = b"d1:ai12453e1:b3:aaa1:c3:bbb1:X10:0123456789e";
        let t = ok(b);
        let e = t.root().unwrap();
        assert_eq!(e.data_section(), b);
        assert_eq!(e.node_type(), NodeType::Dict);
        assert_eq!(e.dict_size(), 4);
        assert_eq!(e.dict_find("a").unwrap().node_type(), NodeType::Int);
        assert_eq!(e.dict_find("a").unwrap().int_value(), 12453);
        assert_eq!(e.dict_find("b").unwrap().string_value(), b"aaa");
        assert_eq!(e.dict_find("b").unwrap().string_length(), 3);
        assert_eq!(e.dict_find("c").unwrap().string_value(), b"bbb");
        assert_eq!(e.dict_find_string_value("X"), Some(&b"0123456789"[..]));
        assert_eq!(e.has_soft_error(), Some("unsorted dictionary key"));
    }

    #[test]
    fn dict_key_novalue() {
        assert_eq!(err(b"d1:ai1e1:be"), BdecodeError { kind: ExpectedValue, pos: 10 });
    }

    #[test]
    fn dict_nonstring_key() {
        assert_eq!(err(b"di5e1:ae"), BdecodeError { kind: ExpectedDigit, pos: 1 });
    }

    #[test]
    fn dict_null_key() {
        let t = ok(b"d3:a\0bi1ee");
        let e = t.root().unwrap();
        assert_eq!(e.dict_size(), 1);
        let d = e.dict_find(b"a\0b").unwrap();
        assert_eq!(d.node_type(), NodeType::Int);
        assert_eq!(d.int_value(), 1);
    }

    #[test]
    fn dict_unordered_keys() {
        for b in [&b"d2:abi1e2:aai2ee"[..], b"d2:bai1e2:aai2ee", b"d2:aai1e1:ai2ee"] {
            assert_eq!(ok(b).root().unwrap().has_soft_error(), Some("unsorted dictionary key"));
        }
        for b in [&b"d1:ai1e2:aai2ee"[..], b"d2:aai1e1:bi2ee"] {
            assert_eq!(ok(b).root().unwrap().has_soft_error(), None);
        }
    }

    #[test]
    fn dict_duplicate_key() {
        assert_eq!(
            ok(b"d2:aai1e2:aai2ee").root().unwrap().has_soft_error(),
            Some("duplicate dictionary key")
        );
    }

    #[test]
    fn premature_e() {
        let (t, e) = decode(b"e");
        assert_eq!(e.unwrap().kind, UnexpectedEof);
        assert!(t.root().is_none());
    }

    #[test]
    fn negative_length_prefix() {
        assert_eq!(err(b"-10:foobar"), BdecodeError { kind: ExpectedValue, pos: 0 });
    }

    #[test]
    fn overflow_length_prefix() {
        assert_eq!(err(b"18446744073709551615:foobar"), BdecodeError { kind: Overflow, pos: 19 });
    }

    #[test]
    fn close_overflow_length_prefix() {
        assert_eq!(err(b"99999999:foobar"), BdecodeError { kind: UnexpectedEof, pos: 8 });
    }

    #[test]
    fn overflow_length_prefix2() {
        // libtorrent pretends the buffer is 999999999 bytes; we lower the limit instead.
        let (_, e) = decode_impl(&b"199999999:foobar"[..], DEFAULT_DEPTH_LIMIT, DEFAULT_TOKEN_LIMIT, 8);
        assert_eq!(e, Some(BdecodeError { kind: LimitExceeded, pos: 0 }));
    }

    #[test]
    fn leading_zero_length_prefix() {
        let (t, _) = decode(b"06:foobar");
        assert_eq!(t.root().unwrap().has_soft_error(), Some("leading zero in string length"));
        assert_eq!(ok(b"0:").root().unwrap().has_soft_error(), None);
    }

    #[test]
    fn nodigit_int() {
        assert_eq!(err(b"ie"), BdecodeError { kind: ExpectedDigit, pos: 1 });
    }

    #[test]
    fn minus_int() {
        assert_eq!(err(b"i-e"), BdecodeError { kind: ExpectedDigit, pos: 2 });
    }

    #[test]
    fn interior_minus_int() {
        assert_eq!(err(b"i35412-5633e"), BdecodeError { kind: ExpectedDigit, pos: 6 });
    }

    #[test]
    fn int_overflow() {
        // the parser accepts it; reading it overflows to 0
        let t = ok(b"i18446744073709551615e");
        assert_eq!(t.root().unwrap().int_value(), 0);
        assert_eq!(t.root().unwrap().try_int_value(), None);
    }

    #[test]
    fn int_overflow2() {
        assert_eq!(err(b"i184467440737095516154e"), BdecodeError { kind: Overflow, pos: 22 });
    }

    #[test]
    fn int_truncated() {
        assert_eq!(err(b"i-"), BdecodeError { kind: UnexpectedEof, pos: 2 });
    }

    #[test]
    fn int_leading_zero() {
        assert_eq!(ok(b"i01e").root().unwrap().has_soft_error(), Some("leading zero in integer"));
        assert_eq!(ok(b"i0e").root().unwrap().has_soft_error(), None);
    }

    #[test]
    fn bdecode_error_messages() {
        assert_eq!(Overflow.message(), "integer overflow");
        let e = BdecodeError { kind: ExpectedColon, pos: 3 };
        assert_eq!(e.to_string(), "expected colon in bencoded string (at byte 3)");
    }

    #[test]
    fn int_64bit() {
        assert_eq!(ok(b"i9223372036854775807e").root().unwrap().int_value(), i64::MAX);
        assert_eq!(ok(b"i-9223372036854775807e").root().unwrap().int_value(), -i64::MAX);
    }

    #[test]
    fn int_invalid_digit() {
        assert_eq!(err(b"i92337203t854775807e"), BdecodeError { kind: ExpectedDigit, pos: 9 });
    }

    #[test]
    fn invalid_encoding() {
        let buf: &[u8] = &[
            0x64, 0x31, 0x3a, 0x61, 0x64, 0x32, 0x3a, 0x69, 0x64, 0x32, 0x30, 0x3a, 0x2a, 0x21, 0x19, 0x89,
            0x9f, 0xcd, 0x5f, 0xc9, 0xbc, 0x80, 0xc1, 0x76, 0xfe, 0xe0, 0xc6, 0x84, 0x2d, 0xf6, 0xfc, 0xb8,
            0x39, 0x3a, 0x69, 0x6e, 0x66, 0x6f, 0x5f, 0x68, 0x61, 0xae, 0x68, 0x32, 0x30, 0x3a, 0x14, 0x78,
            0xd5, 0xb0, 0xdc, 0xf6, 0x82, 0x42, 0x32, 0xa0, 0xd6, 0x88, 0xeb, 0x48, 0x57, 0x01, 0x89, 0x40,
            0x4e, 0xbc, 0x65, 0x31, 0x3a, 0x71, 0x39, 0x3a, 0x67, 0x65, 0x74, 0x5f, 0x70, 0x65, 0x65, 0x72,
            0x78, 0xff, 0x3a, 0x74, 0x38, 0x3a, 0xaa, 0xd4, 0xa1, 0x88, 0x7a, 0x8d, 0xc3, 0xd6, 0x31, 0x3a,
            0x79, 0x31, 0xae, 0x71, 0x65, 0,
        ];
        assert!(decode(buf).1.is_some());
    }

    #[test]
    fn depth_limit() {
        let mut b = vec![b'l'; 1024];
        b.extend(std::iter::repeat_n(b'e', 1024));
        let (_, e) = bdecode_partial(&b[..], 100, DEFAULT_TOKEN_LIMIT);
        assert_eq!(e.unwrap().kind, DepthExceeded);
    }

    #[test]
    fn item_limit() {
        let mut b = vec![b'l'];
        while b.len() < 10239 {
            b.extend_from_slice(b"0:");
        }
        b.push(b'e');
        let (_, e) = bdecode_partial(&b[..], 1000, 1000);
        assert_eq!(e.unwrap().kind, LimitExceeded);
    }

    #[test]
    fn unexpected_eof() {
        assert_eq!(err(b"l2:.."), BdecodeError { kind: UnexpectedEof, pos: 5 });
    }

    #[test]
    fn unexpected_eof2() {
        assert_eq!(err(b"l2:..0"), BdecodeError { kind: UnexpectedEof, pos: 6 });
    }

    #[test]
    fn expected_string() {
        assert_eq!(err(b"di2ei0ee"), BdecodeError { kind: ExpectedDigit, pos: 1 });
    }

    #[test]
    fn unexpected_eof_dict_key() {
        assert_eq!(err(b"d1000:..e"), BdecodeError { kind: UnexpectedEof, pos: 5 });
        assert_eq!(err(b"d1000:"), BdecodeError { kind: UnexpectedEof, pos: 5 });
    }

    #[test]
    fn expected_string_dict_key2() {
        assert_eq!(err(b"df00:"), BdecodeError { kind: ExpectedDigit, pos: 1 });
    }

    #[test]
    fn unexpected_eof_int() {
        assert_eq!(err(b"i"), BdecodeError { kind: UnexpectedEof, pos: 1 });
        assert_eq!(err(b"i10"), BdecodeError { kind: UnexpectedEof, pos: 3 });
    }

    #[test]
    fn expected_colon_dict() {
        assert_eq!(err(b"d1000"), BdecodeError { kind: ExpectedColon, pos: 5 });
    }

    #[test]
    fn empty_string() {
        let (t, e) = decode(b"");
        assert_eq!(e.unwrap().kind, UnexpectedEof);
        assert!(t.root().is_none());
    }

    #[test]
    fn partial_string() {
        assert_eq!(err(b"100:.."), BdecodeError { kind: UnexpectedEof, pos: 3 });
    }

    #[test]
    fn list_ints() {
        let mut buf = String::from("l");
        for i in 0..1000 {
            buf += &format!("i{i}e");
        }
        buf.push('e');
        let t = ok(buf.as_bytes());
        let e = t.root().unwrap();
        assert_eq!(e.node_type(), NodeType::List);
        assert_eq!(e.list_size(), 1000);
        for (i, item) in e.list_iter().enumerate() {
            assert_eq!(item.int_value(), i as i64);
        }
        assert_eq!(e.list_int_value_at(999), Some(999));
    }

    #[test]
    fn dict_ints() {
        let mut buf = String::from("d");
        for i in 0..1000 {
            buf += &format!("4:{i:04}i{i}e");
        }
        buf.push('e');
        let t = ok(buf.as_bytes());
        let e = t.root().unwrap();
        assert_eq!(e.node_type(), NodeType::Dict);
        assert_eq!(e.dict_size(), 1000);
        for i in 0..1000 {
            assert_eq!(e.dict_find_int_value(format!("{i:04}")), Some(i));
        }
    }

    #[test]
    fn dict_at() {
        let t = ok(b"d3:fooi1e3:bari2ee");
        let e = t.root().unwrap();
        assert_eq!(e.dict_size(), 2);
        let (k, v) = e.dict_at(0).unwrap();
        assert_eq!((k, v.int_value()), (&b"foo"[..], 1));
        let (k, v) = e.dict_at(1).unwrap();
        assert_eq!((k, v.int_value()), (&b"bar"[..], 2));
        assert!(e.dict_at(2).is_none());
    }

    #[test]
    fn string_ptr() {
        let t = ok(b"l3:fooe");
        let e = t.root().unwrap();
        assert_eq!(e.list_size(), 1);
        assert_eq!(e.list_at(0).unwrap().string_offset(), 3);
        assert_eq!(e.list_at(0).unwrap().string_length(), 3);
    }

    #[test]
    fn exceed_buf_limit() {
        let (t, e) = decode_impl(&b"l3:fooe"[..], DEFAULT_DEPTH_LIMIT, DEFAULT_TOKEN_LIMIT, 6);
        assert_eq!(e.unwrap().kind, LimitExceeded);
        assert!(t.root().is_none());
    }

    #[test]
    fn parse_int_basic() {
        let b = b"1234567890e";
        let mut val = 0;
        assert_eq!(parse_int(b, b'e', &mut val), (b.len() - 1, None));
        assert_eq!(val, 1234567890);
    }

    #[test]
    fn invalid_digit() {
        let mut val = 0;
        assert_eq!(parse_int(b"0o", b'e', &mut val), (1, Some(ExpectedDigit)));
    }

    #[test]
    fn parse_int_overflow() {
        let mut val = 0;
        assert_eq!(parse_int(b"9223372036854775808:", b':', &mut val), (18, Some(Overflow)));
    }

    #[test]
    fn parse_length_overflow() {
        for b in [
            &b"d1:a1919191010:11111"[..],
            b"d2143289344:a4:aaaae",
            b"d214328934114:a4:aaaae",
            b"d9205357638345293824:a4:aaaae",
            b"d1:a9205357638345293824:11111",
        ] {
            assert_eq!(err(b).kind, UnexpectedEof, "{}", String::from_utf8_lossy(b));
        }
    }

    #[test]
    fn expected_colon_string() {
        let mut val = 0;
        assert_eq!(parse_int(b"928", b':', &mut val), (3, None));
    }

    #[test]
    fn dict_find_funs() {
        // a: int, b: string, c: list, d: dict
        let t = ok(b"d1:ai1e1:b3:foo1:cli1ei2ee1:dd1:xi1eee");
        let e = t.root().unwrap();
        assert_eq!(e.node_type(), NodeType::Dict);

        assert_eq!(e.dict_find_int_value("a"), Some(1));
        assert!(e.dict_find_int("a").is_some());
        assert_eq!(e.dict_find_int_value("b").unwrap_or(-10), -10);
        assert_eq!(e.dict_find_int_value("x").unwrap_or(-10), -10);
        assert!(e.dict_find_int("b").is_none());
        assert!(e.dict_find_int("x").is_none());

        assert_eq!(e.dict_find_string_value("b"), Some(&b"foo"[..]));
        assert!(e.dict_find_string("b").is_some());
        assert_eq!(e.dict_find_string_value("c").unwrap_or(b"blah"), b"blah");
        assert_eq!(e.dict_find_string_value("x").unwrap_or(b"blah"), b"blah");
        assert!(e.dict_find_string("c").is_none());

        let c = e.dict_find_list("c").unwrap();
        assert_eq!(c.list_size(), 2);
        assert_eq!(c.list_int_value_at(0), Some(1));
        assert_eq!(c.list_int_value_at(1), Some(2));
        assert!(e.dict_find_list("d").is_none());

        let d = e.dict_find_dict("d").unwrap();
        assert_eq!(d.dict_find_int_value("x"), Some(1));
        assert_eq!(d.dict_find_int_value("y").unwrap_or(-10), -10);
        assert!(e.dict_find_dict("c").is_none());
        assert!(e.dict_find_dict(String::from("x")).is_none());

        assert_eq!(e.dict_size(), 4);
        assert_eq!(e.dict_at(0).unwrap().0, b"a");
        assert_eq!(e.dict_at(0).unwrap().1.int_value(), 1);
        assert_eq!(e.dict_at(1).unwrap().1.string_value(), b"foo");
        assert_eq!(e.dict_at(2).unwrap().1.node_type(), NodeType::List);
        assert_eq!(e.dict_at(3).unwrap().1.node_type(), NodeType::Dict);
    }

    #[test]
    fn list_at_funs() {
        // int, string, list, dict
        let t = ok(b"li1e3:fooli1ei2eed1:xi1eee");
        let e = t.root().unwrap();
        assert_eq!(e.list_int_value_at(0), Some(1));
        assert_eq!(e.list_int_value_at(1).unwrap_or(-10), -10);
        assert_eq!(e.list_string_value_at(1), Some(&b"foo"[..]));
        assert_eq!(e.list_string_value_at(2).unwrap_or(b"blah"), b"blah");
        let l = e.list_at(2).unwrap();
        assert_eq!(l.list_size(), 2);
        assert_eq!(l.list_int_value_at(0), Some(1));
        assert_eq!(l.list_int_value_at(1), Some(2));
        let d = e.list_at(3).unwrap();
        assert_eq!(d.dict_size(), 1);
        assert_eq!(d.dict_find_int_value("x"), Some(1));
        assert_eq!(d.dict_find_int_value("y"), None);
        assert_eq!(e.list_size(), 4);
        // reverse order
        assert_eq!(e.list_at(3).unwrap().node_type(), NodeType::Dict);
        assert_eq!(e.list_at(2).unwrap().node_type(), NodeType::List);
        assert_eq!(e.list_string_value_at(1), Some(&b"foo"[..]));
        assert_eq!(e.list_int_value_at(0), Some(1));
        assert!(e.list_at(4).is_none());
    }

    #[test]
    fn dict_find_last() {
        let t = ok(b"d1:ai1e1:b3:foo1:cli1ei2ee1:dd1:xi1eee");
        assert_eq!(t.root().unwrap().dict_find("d").unwrap().node_type(), NodeType::Dict);
    }

    #[test]
    fn print_entry_list() {
        let t = ok(b"li1e3:fooli1ei2eed1:xi1eee");
        assert_eq!(pe(&t), "[ 1, 'foo', [ 1, 2 ], { 'x': 1 } ]");
    }

    #[test]
    fn print_entry_dict() {
        let t = ok(b"d1:ai1e1:b3:foo1:cli1ei2ee1:dd1:xi1eee");
        assert_eq!(pe(&t), "{ 'a': 1, 'b': 'foo', 'c': [ 1, 2 ], 'd': { 'x': 1 } }");
    }

    #[test]
    fn subtree_nodes_outlive_nothing_but_their_tree() {
        // libtorrent's swap/copy_root/non_owning tests exercise C++ ownership; in Rust, nodes
        // borrow the tree, so check that subtree handles behave like independent values.
        let t1 = ok(b"d1:ai1e1:b3:foo1:cli1ei2ee1:dd1:xi1eee");
        let t2 = ok(b"li1e3:fooli1ei2eed1:xi1eee");
        let e1 = t1.root().unwrap().dict_find("c").unwrap().list_at(0).unwrap();
        let e2 = t2.root().unwrap().list_at(1).unwrap();
        let (a, b) = (e2, e1);
        assert_eq!(print_entry(a, false, 0), "'foo'");
        assert_eq!(print_entry(b, false, 0), "1");
        let copy = t1.clone();
        drop(t1);
        assert_eq!(copy.root().unwrap().dict_size(), 4);
    }

    #[test]
    fn partial_parse() {
        let b = b"d1:ai1e1:b3:foo1:cli1ei2ee1:dd1:xi1-eee";
        let (t, e) = decode(b);
        assert_eq!(e.unwrap().pos, 35);
        assert_eq!(t.root().unwrap().node_type(), NodeType::Dict);
        assert_eq!(pe(&t), "{ 'a': 1, 'b': 'foo', 'c': [ 1, 2 ], 'd': { 'x': {} } }");
    }

    #[test]
    fn partial_parse2() {
        let (t, e) = decode(b"d1:ai1e1:b3:foo1:cli1ei2ee1:d-d1:xi1eee");
        assert_eq!(e.unwrap().pos, 29);
        assert_eq!(pe(&t), "{ 'a': 1, 'b': 'foo', 'c': [ 1, 2 ], 'd': {} }");
    }

    #[test]
    fn partial_parse3() {
        let (t, e) = decode(b"d1:ai1e1:b3:foo1:cli1ei2ee-1:dd1:xi1eee");
        assert_eq!(e.unwrap().pos, 26);
        assert_eq!(pe(&t), "{ 'a': 1, 'b': 'foo', 'c': [ 1, 2 ] }");
    }

    #[test]
    fn partial_parse4() {
        let (t, e) = decode(b"d1:ai1e1:b3:foo1:cli1e-i2ee1:dd1:xi1eee");
        assert_eq!(e.unwrap().pos, 22);
        assert_eq!(pe(&t), "{ 'a': 1, 'b': 'foo', 'c': [ 1 ] }");
    }

    #[test]
    fn partial_tree_is_structurally_valid() {
        // found by tests/robustness.rs: a dict waiting for a value inside another dict
        let (t, e) = decode(b"d1:ad1:b1:x1:cX");
        assert_eq!(e.unwrap(), BdecodeError { kind: ExpectedValue, pos: 14 });
        let root = t.root().unwrap();
        assert_eq!(root.has_soft_error(), None);
        assert_eq!(root.dict_size(), 1);
        let inner = root.dict_find("a").unwrap();
        assert_eq!(inner.dict_size(), 2);
        assert!(inner.dict_find("c").unwrap().is_dict());
        assert_eq!(pe(&t), "{ 'a': { 'b': 'x', 'c': {} } }");
        // every node's data section stays inside the buffer
        assert_eq!(root.data_section(), b"d1:ad1:b1:x1:c");
    }

    #[test]
    fn partial_parse_string() {
        assert_eq!(err(b"55").pos, 2);
    }

    #[test]
    fn rebind_buffer() {
        // libtorrent switch_underlying_buffer
        let b1 = b"d1:ai1e1:b3:foo1:cli1e-i2ee1:dd1:xi1eee".to_vec();
        let (t, e) = decode(&b1);
        assert_eq!(e.unwrap().pos, 22);
        let s1 = pe(&t);
        let owned: std::sync::Arc<[u8]> = b1.clone().into();
        let t2 = t.rebind(owned);
        assert_eq!(t2.root().map(|n| print_entry(n, false, 0)).unwrap(), s1);
    }

    #[test]
    fn long_string_100000000() {
        // header "100000000:" is 10 bytes: exercises the long-string token form
        let mut input = b"100000000:".to_vec();
        input.resize(10 + 100_000_000, b'_');
        let t = ok(&input);
        let e = t.root().unwrap();
        assert_eq!(e.node_type(), NodeType::Str);
        assert_eq!(e.string_length(), 100_000_000);
        assert_eq!(e.string_offset(), 10);
    }

    #[test]
    fn long_string_99999999() {
        let mut input = b"99999999:".to_vec();
        input.resize(9 + 99_999_999, b'_');
        let t = ok(&input);
        assert_eq!(t.root().unwrap().string_length(), 99_999_999);
    }

    #[test]
    fn data_offset() {
        let t = ok(b"li1e3:fooli1ei2eed1:xi1eee");
        let e = t.root().unwrap();
        assert_eq!(e.data_offset(), 0);
        let offs: Vec<usize> = e.list_iter().map(|n| n.data_offset()).collect();
        assert_eq!(offs, [1, 4, 9, 17]);
    }

    #[test]
    fn string_offset() {
        let t = ok(b"l3:foo3:bare");
        let e = t.root().unwrap();
        assert_eq!(e.list_at(0).unwrap().string_offset(), 3);
        assert_eq!(e.list_at(1).unwrap().string_offset(), 8);
    }

    #[test]
    fn dict_at_node() {
        let t = ok(b"d3:foo3:bar4:test4:teste");
        let e = t.root().unwrap();
        let (k0, v0) = e.dict_at_node(0).unwrap();
        let (k1, v1) = e.dict_at_node(1).unwrap();
        assert_eq!((k0.string_offset(), v0.string_offset()), (3, 8));
        assert_eq!((k1.string_offset(), v1.string_offset()), (13, 19));
    }

    #[test]
    fn type_mismatch_is_lenient() {
        let t = ok(b"i5e");
        let n = t.root().unwrap();
        assert_eq!(n.list_size(), 0);
        assert!(n.dict_find("x").is_none());
        assert_eq!(n.string_value(), b"");
        assert_eq!(n.str_value(), None);
    }

    #[test]
    fn token_is_eight_bytes() {
        assert_eq!(std::mem::size_of::<Token>(), 8);
    }
}
