// Ported from libtorrent-rasterbar 2.0.11: src/write_resume_data.cpp (`write_torrent_file`,
// `build_tracker_list`)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: takes the torrent's parts (metadata, trackers, web seeds) instead of an
// `add_torrent_params`; v2 piece layers are written when the metadata still holds them and left
// out otherwise (libtorrent's `allow_missing_piece_layer`); DHT nodes are never written.

//! A `.torrent` file for a torrent in the session (libtorrent `write_torrent_file`): the info
//! dictionary verbatim, so the info-hash is unchanged, plus the trackers and web seeds in use.

use super::TorrentInfo;
use crate::bencode::Entry;

/// Bencodes a `.torrent` for `ti` with `trackers` as (URL, tier), in tier order.
pub fn write_torrent_file(
    ti: &TorrentInfo,
    trackers: &[(String, u32)],
    url_seeds: &[String],
    http_seeds: &[String],
) -> Vec<u8> {
    let mut ret = Entry::new_dict();
    *ret.key("info") = Entry::Preformatted(ti.info_section().to_vec());
    if !ti.comment().is_empty() {
        *ret.key("comment") = ti.comment().into();
    }
    if let Some(date) = ti.creation_date().filter(|&d| d != 0) {
        *ret.key("creation date") = Entry::Int(date);
    }
    if !ti.creator().is_empty() {
        *ret.key("created by") = ti.creator().into();
    }
    if ti.v2() {
        let fs = ti.orig_files();
        let mut layers = Entry::new_dict();
        for f in 0..fs.num_files() {
            if fs.pad_file_at(f) || fs.file_size(f) <= ti.piece_length() as u64 {
                continue;
            }
            let layer = ti.piece_layer(f);
            if !layer.is_empty() {
                *layers.key(fs.root_bytes(f)) = Entry::Bytes(layer.to_vec());
            }
        }
        if layers.as_dict().is_some_and(|d| !d.is_empty()) {
            *ret.key("piece layers") = layers;
        }
    }
    let list = |v: &[String]| Entry::List(v.iter().map(|s| s.as_str().into()).collect());
    if !url_seeds.is_empty() {
        *ret.key("url-list") = list(url_seeds);
    }
    if !http_seeds.is_empty() {
        *ret.key("httpseeds") = list(http_seeds);
    }
    if !ti.similar_torrents().is_empty() && !info_has(ti, "similar") {
        *ret.key("similar") =
            Entry::List(ti.similar_torrents().iter().map(|h| Entry::Bytes(h.as_bytes().to_vec())).collect());
    }
    if !ti.collections().is_empty() && !info_has(ti, "collections") {
        *ret.key("collections") = list(ti.collections());
    }
    match trackers {
        [] => {}
        [(url, _)] => *ret.key("announce") = url.as_str().into(),
        _ => *ret.key("announce-list") = build_tracker_list(trackers),
    }
    ret.bencode()
}

/// libtorrent `build_tracker_list`: one list per tier, tiers in the order they first appear.
fn build_tracker_list(trackers: &[(String, u32)]) -> Entry {
    let mut tiers: Vec<(u32, Vec<Entry>)> = Vec::new();
    for (url, tier) in trackers {
        match tiers.iter_mut().find(|(t, _)| t == tier) {
            Some((_, l)) => l.push(url.as_str().into()),
            None => tiers.push((*tier, vec![url.as_str().into()])),
        }
    }
    Entry::List(tiers.into_iter().map(|(_, l)| Entry::List(l)).collect())
}

/// Whether the info dictionary itself has `key` (then it is not repeated outside it).
fn info_has(ti: &TorrentInfo, key: &str) -> bool {
    ti.info_dict().and_then(|d| d.root().and_then(|r| r.dict_find(key.as_bytes()).map(|_| ()))).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metainfo::{CreateTorrent, FileStorage};

    fn sample() -> TorrentInfo {
        let mut fs = FileStorage::new();
        fs.add_file("t/a.bin", 40_000, Default::default(), 0, "", None).unwrap();
        fs.add_file("t/b.bin", 9_000, Default::default(), 0, "", None).unwrap();
        let mut c = CreateTorrent::new(fs, 16 * 1024, Default::default()).unwrap();
        for i in 0..c.num_pieces() {
            c.set_hash(i, crate::types::Sha1Hash([i as u8 + 1; 20]));
        }
        c.set_comment("hello");
        c.set_creator("tester");
        c.add_tracker("http://one/announce", 0);
        let bytes = c.generate_buf().unwrap();
        TorrentInfo::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn keeps_the_info_hash_and_writes_the_parts_in_use() {
        let ti = sample();
        let trackers = [
            ("http://a/announce".to_owned(), 0),
            ("udp://b:80".to_owned(), 1),
            ("http://c/announce".to_owned(), 0),
        ];
        let out = write_torrent_file(&ti, &trackers, &["http://seed/".to_owned()], &[]);
        let back = TorrentInfo::from_bytes(&out).unwrap();
        assert_eq!(back.info_hashes(), ti.info_hashes());
        assert_eq!(back.comment(), "hello");
        assert_eq!(back.creator(), "tester");
        assert_eq!(back.creation_date(), ti.creation_date());
        // the parser shuffles each tier (BEP 12)
        let mut urls: Vec<(u8, &str)> = back.trackers().iter().map(|t| (t.tier, t.url.as_str())).collect();
        urls.sort();
        assert_eq!(urls, [(0, "http://a/announce"), (0, "http://c/announce"), (1, "udp://b:80")]);
        assert_eq!(back.web_seeds().len(), 1);
        assert_eq!(back.web_seeds()[0].url, "http://seed/");
    }

    #[test]
    fn one_tracker_is_announce_and_none_is_nothing() {
        let ti = sample();
        let out = write_torrent_file(&ti, &[("http://x/a".to_owned(), 3)], &[], &[]);
        assert!(out.windows(8).any(|w| w == b"announce"));
        assert!(!out.windows(13).any(|w| w == b"announce-list"));
        let back = TorrentInfo::from_bytes(&out).unwrap();
        assert_eq!(back.trackers()[0].url, "http://x/a");
        let bare = write_torrent_file(&ti, &[], &[], &[]);
        assert!(!bare.windows(8).any(|w| w == b"announce"));
        assert_eq!(TorrentInfo::from_bytes(&bare).unwrap().info_hashes(), ti.info_hashes());
    }
}
