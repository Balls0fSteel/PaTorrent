// Ported from libtorrent-rasterbar 2.0.11: src/xml_parse.cpp, include/libtorrent/xml_parse.hpp,
// test/test_xml.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the callback gets a `Token` enum instead of an int plus two string views.

//! The small, forgiving XML scanner UPnP device descriptions and SOAP replies go through. It
//! reports tags, strings and attributes as it meets them; it does not build a tree and it does
//! not reject malformed input beyond a `ParseError` token.

use crate::util::string::{is_space, string_begins_no_case};

/// What the scanner found (libtorrent `xml_start_tag` and friends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token<'a> {
    StartTag(&'a str),
    EndTag(&'a str),
    /// `<foo/>`.
    EmptyTag(&'a str),
    /// `<?xml ... ?>`.
    DeclarationTag(&'a str),
    Comment(&'a str),
    /// Text between tags, or CDATA contents.
    String(&'a str),
    Attribute(&'a str, &'a str),
    ParseError(&'static str),
    /// Attribute text without an `=`: reported as is, not as a key-value pair.
    TagContent(&'a str),
}

/// libtorrent `xml_parse`.
pub fn parse<'a>(input: &'a str, mut callback: impl FnMut(Token<'a>)) {
    let b = input.as_bytes();
    let end = b.len();
    // slices of `input` by byte offsets; every offset is at an ASCII byte or `end`, so they
    // are char boundaries
    let s = |from: usize, to: usize| -> &'a str { &input[from..to] };
    let mut p = 0;
    while p != end {
        let mut start = p;
        // look for tag start
        while p != end && b[p] != b'<' {
            p += 1;
        }
        if p != start {
            callback(Token::String(s(start, p)));
        }
        if p == end {
            break;
        }
        // skip '<'
        p += 1;
        if p != end && p + 8 < end && string_begins_no_case(b"![CDATA[", &b[p..]) {
            // CDATA. match '![CDATA['
            p += 8;
            start = p;
            while p != end && !string_begins_no_case(b"]]>", &b[p - 2..]) {
                p += 1;
            }
            // parse error
            if p == end {
                callback(Token::ParseError("unexpected end of file"));
                break;
            }
            callback(Token::String(s(start, p - 2)));
            p += 1;
            continue;
        }

        // parse the name of the tag.
        start = p;
        while p != end && b[p] != b'>' && !is_space(b[p]) {
            p += 1;
        }
        let tag_name_end = p;

        // skip the attributes for now
        while p != end && b[p] != b'>' {
            p += 1;
        }

        // parse error
        if p == end {
            callback(Token::ParseError("unexpected end of file"));
            break;
        }
        debug_assert_eq!(b[p], b'>');

        let mut tag_end = p;
        if b[start] == b'/' {
            start += 1;
            callback(Token::EndTag(s(start, tag_name_end)));
        } else if b[p - 1] == b'/' {
            callback(Token::EmptyTag(s(start, (tag_name_end - start).min(p - start - 1) + start)));
            tag_end = p - 1;
        } else if b[start] == b'?' && b[p - 1] == b'?' {
            start += 1;
            // `<?>`: the one '?' is both the first and the last byte, which libtorrent's
            // `p - start - 1` turns into a huge length
            let len = (tag_name_end - start).min((p - start).saturating_sub(1));
            callback(Token::DeclarationTag(s(start, start + len)));
            tag_end = p - 1;
        } else if start + 5 < p && &b[start..start + 3] == b"!--" && &b[p - 2..p] == b"--" {
            start += 3;
            // libtorrent cuts the comment at `tag_name_end - 2`, which lies before `start` when
            // the comment holds whitespace (an unsigned underflow there); the whole text between
            // the dashes is what a comment is
            callback(Token::Comment(s(start, (p - 2).max(start))));
            p += 1;
            continue;
        } else {
            callback(Token::StartTag(s(start, tag_name_end)));
        }

        // parse attributes
        let mut i = tag_name_end;
        while i < tag_end {
            // find start of attribute name
            while i != tag_end && is_space(b[i]) {
                i += 1;
            }
            if i == tag_end {
                break;
            }
            start = i;
            // find end of attribute name
            while i != tag_end && b[i] != b'=' && !is_space(b[i]) {
                i += 1;
            }
            let name_end = i;

            // look for equality sign
            while i != tag_end && b[i] != b'=' {
                i += 1;
            }

            // no equality sign found. Report this as xml_tag_content
            // instead of a series of key value pairs
            if i == tag_end {
                callback(Token::TagContent(s(start, i)));
                break;
            }

            i += 1;
            while i != tag_end && is_space(b[i]) {
                i += 1;
            }
            // check for parse error (values must be quoted)
            if i == tag_end || (b[i] != b'\'' && b[i] != b'"') {
                callback(Token::ParseError("unquoted attribute value"));
                break;
            }
            let quote = b[i];
            i += 1;
            let val_start = i;
            while i != tag_end && b[i] != quote {
                i += 1;
            }
            // parse error (missing end quote)
            if i == tag_end {
                callback(Token::ParseError("missing end quote on attribute"));
                break;
            }
            callback(Token::Attribute(s(start, name_end), s(val_start, i)));
            i += 1;
        }
        p += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libtorrent `test_xml.cpp`'s `parser_callback` encoding: one letter per token.
    fn render(input: &str) -> String {
        let mut out = String::new();
        parse(input, |t| match t {
            Token::StartTag(s) => out.push_str(&format!("B{s}")),
            Token::EndTag(s) => out.push_str(&format!("F{s}")),
            Token::EmptyTag(s) => out.push_str(&format!("E{s}")),
            Token::DeclarationTag(s) => out.push_str(&format!("D{s}")),
            Token::Comment(s) => out.push_str(&format!("C{s}")),
            Token::String(s) => out.push_str(&format!("S{s}")),
            Token::Attribute(k, v) => {
                assert!(!v.is_empty());
                out.push_str(&format!("A{k}V{v}"))
            }
            Token::ParseError(s) => out.push_str(&format!("P{s}")),
            Token::TagContent(s) => out.push_str(&format!("T{s}")),
        });
        out
    }

    #[test]
    fn tags() {
        assert_eq!(render("<a>foo<b/>bar</a>"), "BaSfooEbSbarFa");
    }

    #[test]
    fn xml_tag_comment() {
        assert_eq!(
            render(
                "<?xml version = \"1.0\"?><c x=\"1\" \t y=\"3\"/><d foo='bar'></d boo='foo'><!--comment-->"
            ),
            "DxmlAversionV1.0EcAxV1AyV3BdAfooVbarFdAbooVfooCcomment"
        );
    }

    #[test]
    fn empty_tag() {
        assert_eq!(render("<foo/>"), "Efoo");
        assert_eq!(render("<foo  />"), "Efoo");
    }

    #[test]
    fn xml_tag_no_attribute() {
        assert_eq!(render("<?xml?>"), "Dxml");
        assert_eq!(render("<?xml  ?>"), "Dxml");
    }

    #[test]
    fn attribute_missing_quote() {
        assert_eq!(
            render("<a f=1>foo</a f='b>"),
            "BaPunquoted attribute valueSfooFaPmissing end quote on attribute"
        );
    }

    #[test]
    fn attribute_whitespace() {
        assert_eq!(render("<a  f>foo</a  v  >"), "BaTfSfooFaTv  ");
    }

    #[test]
    fn unterminated_cdata() {
        assert_eq!(render("<![CDATA[foo"), "Punexpected end of file");
    }

    #[test]
    fn cdata() {
        assert_eq!(
            render("<![CDATA[verbatim tag that can have > and < in it]]>"),
            "Sverbatim tag that can have > and < in it"
        );
    }

    #[test]
    fn unterminated_tag() {
        assert_eq!(render("<foo"), "Punexpected end of file");
    }

    #[test]
    fn unquoted_attribute_value() {
        assert_eq!(render("<foo a=bar>"), "BfooPunquoted attribute value");
    }

    #[test]
    fn unterminated_attribute() {
        assert_eq!(render("<foo a=\"bar>"), "BfooPmissing end quote on attribute");
    }

    #[test]
    fn unterminated_tag_with_attribute() {
        assert_eq!(render("<foo a=\"bar"), "Punexpected end of file");
    }

    #[test]
    fn comment_with_whitespace() {
        assert_eq!(render("<!-- a b -->"), "C a b ");
    }

    #[test]
    fn non_ascii_text_is_passed_through() {
        assert_eq!(render("<a>héllo wörld</a>"), "BaShéllo wörldFa");
        assert_eq!(render("<a>x</a>€<![CDATA[é]]>"), "BaSxFaS€Sé");
    }

    #[test]
    fn odd_edges_do_not_panic() {
        for s in [
            "<",
            "<>",
            "</",
            "<!--",
            "<!---->",
            "<!--->",
            "<?",
            "<?>",
            "<a =>",
            "<a =\"",
            "< >",
            "<![CDATA[]]>",
        ] {
            render(s);
        }
    }
}
