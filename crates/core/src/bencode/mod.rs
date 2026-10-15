//! Bencoding: the zero-copy decoder (`bdecode`) and the owned value type with its encoder
//! (`Entry`).
//!
//! Ported from libtorrent-rasterbar 2.0.11: src/bdecode.cpp, include/libtorrent/bdecode.hpp,
//! src/entry.cpp, include/libtorrent/entry.hpp, include/libtorrent/bencode.hpp.
//! Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
//! Changes: "none" nodes are `Option::None`; lookups use iterators instead of the per-node
//! `list_at` cache; the deprecated recursive `entry` decoder is not ported.

mod bdecode;
mod entry;

pub use bdecode::{
    BdecodeError, BdecodeErrorKind, Bdecoded, DEFAULT_DEPTH_LIMIT, DEFAULT_TOKEN_LIMIT, DictIter, ListIter,
    Node, NodeType, bdecode, bdecode_partial, bdecode_with_limits, parse_int, print_entry,
};
pub use entry::{Entry, integer_to_str};
