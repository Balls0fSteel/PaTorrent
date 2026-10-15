// Ported from libtorrent-rasterbar 2.0.11: src/torrent_info.cpp (valid_path_character,
// filter_path_character, aux::sanitize_append_path_element)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: '/' is the separator on every platform; Windows-only rules apply under cfg(windows)
// exactly as libtorrent applies them under TORRENT_WINDOWS. Reserved device names are not
// rewritten because the disk layer uses \\?\ paths (libtorrent's default TORRENT_USE_UNC_PATHS).

//! Turning untrusted path elements from metadata into safe, valid UTF-8 path components.

use crate::util::utf8::parse_utf8_codepoint;

/// Characters the target filesystem rejects. Separators are handled separately.
fn valid_path_character(c: u32) -> bool {
    const INVALID: &[u8] = if cfg!(windows) { b"?<>\"|\x08*:" } else { b"" };
    if c < 32 {
        return false;
    }
    if c > 127 {
        return true;
    }
    !INVALID.contains(&(c as u8))
}

/// Characters dropped outright: separators, NUL, and Unicode direction overrides that can disguise
/// file extensions.
fn filter_path_character(c: u32) -> bool {
    const BAD: [u32; 7] = [0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x200e, 0x200f];
    if BAD.contains(&c) {
        return true;
    }
    // libtorrent uses strchr("/\\", c), which also matches the terminating NUL
    matches!(c, 0x2f | 0x5c | 0)
}

/// Appends one untrusted path element to `path` (libtorrent `sanitize_append_path_element`):
/// separators and direction overrides are dropped, invalid UTF-8 and characters the filesystem
/// rejects become '_', "." and ".." disappear, and long names are cut to about 240 bytes while
/// keeping the file extension.
pub fn sanitize_append_path_element(path: &mut String, element: &[u8]) {
    if element == b"." {
        return;
    }
    let added_separator = if path.is_empty() {
        0
    } else {
        path.push('/');
        1
    };
    if element.is_empty() {
        path.push('_');
        return;
    }

    let mut added = 0usize;
    let mut num_dots = 0usize;
    let mut found_extension = false;
    #[cfg(windows)]
    let mut unicode_chars = 0usize;

    let mut i = 0usize;
    while i < element.len() {
        let (cp, seq_len) = parse_utf8_codepoint(&element[i..]);
        let seq_len = seq_len.max(1);
        match cp {
            Some(c) if filter_path_character(c) => {
                i += seq_len;
                continue;
            }
            Some(c) if valid_path_character(c) => {
                let s = std::str::from_utf8(&element[i..i + seq_len]).expect("validated codepoint");
                path.push_str(s);
                if c == '.' as u32 {
                    num_dots += 1;
                }
                added += seq_len;
            }
            _ => {
                // invalid UTF-8 or a character the filesystem rejects
                path.push('_');
                added += 1;
            }
        }
        #[cfg(windows)]
        {
            unicode_chars += 1;
        }
        #[cfg(windows)]
        let too_long = unicode_chars >= 240;
        #[cfg(not(windows))]
        let too_long = added >= 240;

        // keep path elements under 255 characters: past 240, jump to the extension (if any)
        if too_long && !found_extension {
            let lower = (element.len() as isize - 10).max(i as isize);
            let dot = (lower + 1..element.len() as isize).rev().find(|&j| element[j as usize] == b'.');
            let Some(dot) = dot else { break };
            found_extension = true;
            i = dot as usize;
            continue;
        }
        i += seq_len;
    }

    if added == num_dots && added <= 2 {
        // "." or "..": revert everything, including the separator
        path.truncate(path.len() - added - added_separator);
        return;
    }

    #[cfg(windows)]
    {
        // Windows does not allow trailing spaces and dots in file names
        while path.ends_with([' ', '.']) && added > 0 {
            path.pop();
            added -= 1;
        }
        if added == 0 && added_separator == 1 {
            path.pop(); // the separator
            return;
        }
    }

    if path.is_empty() {
        path.push('_');
    }
}

#[cfg(test)]
mod tests {
    //! Ported from libtorrent-rasterbar 2.0.11 test/test_torrent_info.cpp (sanitize_path*).
    //! libtorrent passes C strings, so an input with an embedded NUL ends at the NUL there; those
    //! cases are written with the bytes before the NUL.
    use super::*;

    fn s(parts: &[&[u8]]) -> String {
        let mut p = String::new();
        for part in parts {
            sanitize_append_path_element(&mut p, part);
        }
        p
    }

    const WIN: bool = cfg!(windows);

    #[test]
    fn truncate() {
        let a = b"abcdefghi_".repeat(25);
        let mut b = b"abcdefghi_".repeat(24);
        b.extend_from_slice(b"abcdefghi_abcde.test");
        let expected = format!(
            "{}/{}.test",
            String::from_utf8(b"abcdefghi_".repeat(24)).unwrap(),
            String::from_utf8(b"abcdefghi_".repeat(24)).unwrap()
        );
        assert_eq!(s(&[&a, &b]), expected);
    }

    #[test]
    fn truncate_utf() {
        // 239 ASCII bytes, then a 3-byte em dash crosses the 240 limit
        let mut input = b"abcdefghi_".repeat(20);
        input.extend_from_slice(b"abcdefghi_abcdefghi_abcdefghi_abcdefghi\xE2\x80\x94abcde.jpg");
        let mut expected = b"abcdefghi_".repeat(20);
        expected.extend_from_slice(b"abcdefghi_abcdefghi_abcdefghi_abcdefghi\xE2\x80\x94.jpg");
        assert_eq!(s(&[&input]).into_bytes(), expected);
    }

    #[test]
    fn trailing_dots_and_spaces() {
        assert_eq!(s(&[b"a", b"abc...", b"c"]), if WIN { "a/abc/c" } else { "a/abc.../c" });
        assert_eq!(s(&[b"abc..."]), if WIN { "abc" } else { "abc..." });
        assert_eq!(s(&[b"abc."]), if WIN { "abc" } else { "abc." });
        assert_eq!(s(&[b"a. . ."]), if WIN { "a" } else { "a. . ." });
        assert_eq!(s(&[b"a", b"abc   ", b"c"]), if WIN { "a/abc/c" } else { "a/abc   /c" });
        assert_eq!(s(&[b"abc   "]), if WIN { "abc" } else { "abc   " });
        assert_eq!(s(&[b"abc "]), if WIN { "abc" } else { "abc " });
    }

    #[test]
    fn sanitize_path() {
        assert_eq!(s(&[b""]), "_"); // "\0\0\xed\0\x80" as a C string
        assert_eq!(s(&[b"/a/", b"b", b"c"]), "a/b/c");
        assert_eq!(s(&[b"a...b"]), "a...b");
        assert_eq!(s(&[b"a", b"..", b"c"]), "a/c");
        assert_eq!(s(&[b"a", b".."]), "a");
        assert_eq!(s(&[b"/..", b".", b"c"]), "c");
        assert_eq!(s(&[b"dev:"]), if WIN { "dev_" } else { "dev:" });
        assert_eq!(s(&[b"c:", b"b"]), if WIN { "c_/b" } else { "c:/b" });
        assert_eq!(s(&[b"c:", b".", b"c"]), if WIN { "c_/c" } else { "c:/c" });
        assert_eq!(s(&[b"\\c", b".", b"c"]), "c/c");
        assert_eq!(s(&[b"\x08"]), "_");
        assert_eq!(s(&[b"\x08", b"filename"]), "_/filename");
        assert_eq!(s(&[b"filename", b"\x08"]), "filename/_");
        assert_eq!(s(&[b"abc", b""]), "abc/_");
        assert_eq!(s(&[b"abc", b"   "]), if WIN { "abc" } else { "abc/   " });
        assert_eq!(s(&[b"", b"abc"]), "_/abc");
        assert_eq!(s(&[b"\x08?filename=4"]), if WIN { "__filename=4" } else { "_?filename=4" });
        assert_eq!(s(&[b"filename=4"]), "filename=4");
        // UTF-8: valid sequences kept, truncated / overlong / 5-byte sequences become '_'
        assert_eq!(s(&[b"filename\xc2\xa1"]), "filename\u{a1}");
        assert_eq!(s(&[b"filename\xc2"]), "filename_");
        assert_eq!(s(&[b"filename\xe2\x9f\xb9"]), "filename\u{27f9}");
        assert_eq!(s(&[b"filename\xe2\x9f"]), "filename_");
        assert_eq!(s(&[b"filename\xe2"]), "filename_");
        assert_eq!(s(&[b"filename\xf0\x9f\x92\x88"]), "filename\u{1f488}");
        assert_eq!(s(&[b"filename\xf0\x9f\x92"]), "filename_");
        assert_eq!(s(&[b"filename\xf8\x9f\x9f\x9f\x9ffoobar"]), "filename_foobar");
        assert_eq!(s(&[b"filename\xc0\xae"]), "filename_");
        assert_eq!(s(&[b"filename\xe0\x80\xae"]), "filename_");
        assert_eq!(s(&[b"filename\xf0\x80\x80\xae"]), "filename_");
        // every character filtered: no underscore
        assert_eq!(s(&[b"//\\"]), "");
        // direction overrides are removed
        assert_eq!(s(&[b"foo\xe2\x80\x8ebar"]), "foobar");
        assert_eq!(s(&[b"foo\xe2\x80\xabbar"]), "foobar");
    }

    #[test]
    fn nul_bytes_are_dropped() {
        // libtorrent's filter matches NUL (strchr), so embedded NULs vanish
        assert_eq!(s(&[b"\0foo"]), "foo");
        assert_eq!(s(&[b"\0\0\0\0"]), "");
    }

    #[test]
    fn colon() {
        assert_eq!(s(&[b"foo:bar"]), if WIN { "foo_bar" } else { "foo:bar" });
    }
}
