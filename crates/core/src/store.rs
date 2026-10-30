//! Session persistence (ARCHITECTURE.md "Persistence", PLAN.md D5): one resume database and one
//! metadata pack per session directory, instead of two files per torrent.
//!
//! * `resume.db`: a bencoded dictionary `{"file-format", "file-version", "torrents": {key:
//!   record}}`, where each record is a libtorrent fast-resume dictionary without the `info` key.
//!   It is rewritten atomically (temporary file + rename) only when the journal has grown.
//! * `resume.log`: the journal of records changed since `resume.db` was written, appended at
//!   each save. Each entry is `"LTR1"`, a little-endian `u16` key length, the key, a
//!   little-endian `u32` length and the record (`u32::MAX` and no record for a removed torrent).
//!   At startup it is replayed over `resume.db`; once it passes half of `resume.db` (and 4 MiB),
//!   both are folded into a new `resume.db` and the journal starts over. So a save writes only
//!   the torrents that changed, and the store keeps a hash per record, not the records.
//! * `metadata.pack`: append-only info dictionaries. Each record is `"LTI1"`, a little-endian
//!   `u16` key length, the key (info-hash hex), a little-endian `u32` length and the bencoded
//!   info dictionary. Records are read only when a torrent starts, so stopped torrents never
//!   hold their piece hashes in memory. A v2 torrent's piece layers, which live outside the
//!   info dictionary, are a second record under `<key>.layers` (a bencoded "piece layers"
//!   dictionary). The pack is compacted when more than half of it is dead.
//!
//! New code (libtorrent stores resume data through the client).

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::hash::{DefaultHasher, Hasher};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::bencode::{DEFAULT_DEPTH_LIMIT, Entry, bdecode, bdecode_with_limits};
use crate::error::Error;
use crate::resume::ResumeData;

const RESUME_DB: &str = "resume.db";
const RESUME_LOG: &str = "resume.log";
const METADATA_PACK: &str = "metadata.pack";
const PACK_MAGIC: &[u8; 4] = b"LTI1";
const LOG_MAGIC: &[u8; 4] = b"LTR1";
/// The journal entry length of a removed torrent.
const REMOVED: u32 = u32::MAX;
/// The journal is folded into `resume.db` once it is at least this big and half its size.
const COMPACT_LOG_MIN: u64 = 4 * 1024 * 1024;
const DB_FORMAT: &str = "patorrent resume db";
/// Suffix of the pack key that holds a torrent's v2 piece layers.
const LAYERS_SUFFIX: &str = ".layers";
/// The journal buffer is freed after a flush when it grew past this.
const PENDING_KEEP: usize = 64 * 1024;

/// Location of an info dictionary in the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackLoc {
    offset: u64,
    len: u32,
}

impl PackLoc {
    /// No info dictionary in the pack (a real one is at least `de`).
    const NONE: PackLoc = PackLoc { offset: 0, len: 0 };

    fn is_some(self) -> bool {
        self.len > 0
    }
}

/// A store key held in binary: the info-hash hex of a v1 or a v2 torrent (40 or 64 lowercase
/// digits) in 20 or 32 bytes, anything else (a foreign or damaged file) as the text. Every torrent
/// has one in memory, so a `String` (24 bytes plus a 40-byte heap block) was a good part of the
/// store's share per stopped torrent.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum StoreKey {
    V1([u8; 20]),
    V2([u8; 32]),
    Other(Box<str>),
}

impl StoreKey {
    fn new(key: &str) -> StoreKey {
        fn hex<const N: usize>(s: &[u8]) -> Option<[u8; N]> {
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            let mut out = [0u8; N];
            for (o, pair) in out.iter_mut().zip(s.chunks_exact(2)) {
                *o = (digit(pair[0])? << 4) | digit(pair[1])?;
            }
            Some(out)
        }
        let b = key.as_bytes();
        match b.len() {
            40 => hex(b).map(StoreKey::V1),
            64 => hex(b).map(StoreKey::V2),
            _ => None,
        }
        .unwrap_or_else(|| StoreKey::Other(key.into()))
    }
}

impl std::fmt::Display for StoreKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes: &[u8] = match self {
            StoreKey::V1(b) => b,
            StoreKey::V2(b) => b,
            StoreKey::Other(s) => return f.write_str(s),
        };
        bytes.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// What the store keeps per saved torrent.
#[derive(Clone, Copy, Debug)]
struct Saved {
    /// A hash of the encoded record (unchanged records are not written).
    hash: u64,
    /// The info dictionary in the pack, if there.
    info: PackLoc,
}

pub(crate) struct Store {
    dir: PathBuf,
    /// Every saved torrent by key: its record's hash and where its info dictionary is (one map
    /// for both: they have the same keys, an info dictionary being live only with a record).
    saved: HashMap<StoreKey, Saved>,
    /// v2 piece layers in the pack, by the key of their torrent (only v2 torrents have them).
    layers: HashMap<StoreKey, PackLoc>,
    /// Journal entries not written yet.
    pending: Vec<u8>,
    /// Bytes of `resume.log` and of `resume.db`.
    log_len: u64,
    db_len: u64,
    pack: Option<File>,
    pack_len: u64,
}

impl Store {
    /// Opens (or creates) the store in `dir` and returns every saved torrent, in no particular
    /// order. Unreadable records are skipped and reported in the second list.
    pub fn open(dir: &Path) -> Result<(Store, Vec<ResumeData>, Vec<String>), Error> {
        crate::util::private_file::create_dir_all(dir)?;
        let mut store = Store {
            dir: dir.to_owned(),
            saved: HashMap::new(),
            layers: HashMap::new(),
            pending: Vec::new(),
            log_len: 0,
            db_len: 0,
            pack: None,
            pack_len: 0,
        };
        let (db, log) = store.read_files()?;
        let records = store.read_records(db.as_deref(), &log)?;
        let mut torrents = Vec::with_capacity(records.len());
        let mut errors = Vec::new();
        store.saved.reserve(records.len());
        for (key, bytes) in &records {
            let decoded =
                bdecode(bytes).map_err(|e| e.kind.message().to_owned()).and_then(|doc| match doc.root() {
                    Some(root) => ResumeData::from_node(root).map_err(|e| e.to_string()),
                    None => Err("empty record".to_owned()),
                });
            match decoded {
                Ok(r) => {
                    store
                        .saved
                        .insert(StoreKey::new(key), Saved { hash: record_hash(bytes), info: PackLoc::NONE });
                    torrents.push(r);
                }
                Err(e) => errors.push(format!("{key}: {e}")),
            }
        }
        if store.log_len >= COMPACT_LOG_MIN && store.log_len > store.db_len / 2 {
            store.write_db(&records)?;
        }
        drop(records);
        drop((db, log));
        store.open_pack()?;
        let live = store.saved.values().map(|s| s.info).chain(store.layers.values().copied());
        let live = live.filter(|l| l.is_some()).map(|l| l.len as u64 + 64).sum::<u64>();
        if store.pack_len > 1024 * 1024 && store.pack_len > 2 * live {
            store.compact()?;
        }
        Ok((store, torrents, errors))
    }

    /// `resume.db` and `resume.log` as they are on disk (`None` and empty when missing).
    fn read_files(&mut self) -> Result<(Option<Vec<u8>>, Vec<u8>), Error> {
        let read = |name: &str| match std::fs::read(self.dir.join(name)) {
            Ok(buf) => Ok(Some(buf)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        };
        let db = read(RESUME_DB)?;
        let log = read(RESUME_LOG)?.unwrap_or_default();
        self.db_len = db.as_ref().map_or(0, |d| d.len() as u64);
        Ok((db, log))
    }

    /// Every saved record: `db` (`resume.db`) with `log` (`resume.log`) replayed over it, the
    /// records left where they are in the two buffers (at 100,000 torrents a copy of each was
    /// 87 MB more at startup). A torn journal entry (a crash while appending) is cut off.
    fn read_records<'a>(
        &mut self,
        db: Option<&'a [u8]>,
        log: &'a [u8],
    ) -> Result<BTreeMap<String, &'a [u8]>, Error> {
        let mut records = BTreeMap::new();
        if let Some(db) = db {
            parse_db(db, &mut records)?;
        }
        let pos = replay_log(log, &mut records);
        if pos < log.len() {
            OpenOptions::new().write(true).open(self.dir.join(RESUME_LOG))?.set_len(pos as u64)?;
        }
        self.log_len = pos as u64;
        Ok(records)
    }

    /// Writes `records` as the new `resume.db` (atomically: temporary file, flush, rename),
    /// then empties the journal they include.
    fn write_db(&mut self, records: &BTreeMap<String, &[u8]>) -> Result<(), Error> {
        let mut torrents = Entry::new_dict();
        // records that did not load are left out, as they were never kept
        for (k, v) in records.iter().filter(|(k, _)| self.saved.contains_key(&StoreKey::new(k))) {
            *torrents.key(k) = Entry::Preformatted(v.to_vec());
        }
        let mut root = Entry::new_dict();
        *root.key("file-format") = Entry::Bytes(DB_FORMAT.as_bytes().to_vec());
        *root.key("file-version") = Entry::Int(1);
        *root.key("torrents") = torrents;
        let data = root.bencode();
        let tmp = self.dir.join(format!("{RESUME_DB}.tmp"));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.dir.join(RESUME_DB))?;
        self.db_len = data.len() as u64;
        // a crash before this replays a journal already folded in: the same records again
        match OpenOptions::new().write(true).open(self.dir.join(RESUME_LOG)) {
            Ok(f) => f.set_len(0)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        self.log_len = 0;
        Ok(())
    }

    fn open_pack(&mut self) -> Result<(), Error> {
        let path = self.dir.join(METADATA_PACK);
        let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
        let len = f.metadata()?.len();
        let mut pos = 0u64;
        let mut header = [0u8; 6];
        // Scan the record headers; a torn final record (crash while appending) is cut off.
        while pos + 6 <= len {
            f.seek(SeekFrom::Start(pos))?;
            f.read_exact(&mut header)?;
            if &header[..4] != PACK_MAGIC {
                break;
            }
            let klen = u16::from_le_bytes([header[4], header[5]]) as u64;
            if pos + 6 + klen + 4 > len {
                break;
            }
            let mut key = vec![0u8; klen as usize];
            f.read_exact(&mut key)?;
            let mut l = [0u8; 4];
            f.read_exact(&mut l)?;
            let dlen = u32::from_le_bytes(l);
            let data_off = pos + 6 + klen + 4;
            if data_off + dlen as u64 > len {
                break;
            }
            let key = String::from_utf8_lossy(&key);
            let loc = PackLoc { offset: data_off, len: dlen };
            // Later records win; only keys with a resume record are live.
            match key.strip_suffix(LAYERS_SUFFIX) {
                Some(base) => {
                    let base = StoreKey::new(base);
                    if self.saved.contains_key(&base) {
                        self.layers.insert(base, loc);
                    }
                }
                None => {
                    if let Some(s) = self.saved.get_mut(&StoreKey::new(&key)) {
                        s.info = loc;
                    }
                }
            }
            pos = data_off + dlen as u64;
        }
        if pos < len {
            f.set_len(pos)?;
        }
        self.pack_len = pos;
        self.pack = Some(f);
        Ok(())
    }

    fn compact(&mut self) -> Result<(), Error> {
        let tmp = self.dir.join(format!("{METADATA_PACK}.tmp"));
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let infos = self.saved.iter().filter(|(_, s)| s.info.is_some()).map(|(k, s)| (k.to_string(), s.info));
        let layers = self.layers.iter().map(|(k, &l)| (format!("{k}{LAYERS_SUFFIX}"), l));
        let locs: Vec<(String, PackLoc)> = infos.chain(layers).collect();
        for (k, loc) in locs {
            entries.push((k, self.read_pack(loc)?));
        }
        {
            let mut out = File::create(&tmp)?;
            for (k, d) in &entries {
                out.write_all(&pack_record(k, d))?;
            }
            out.sync_all()?;
        }
        self.pack = None;
        std::fs::rename(&tmp, self.dir.join(METADATA_PACK))?;
        for s in self.saved.values_mut() {
            s.info = PackLoc::NONE;
        }
        self.layers.clear();
        // Records were rewritten for live keys only; reindex them.
        self.open_pack()?;
        Ok(())
    }

    pub fn has_metadata(&self, key: &str) -> bool {
        self.saved.get(&StoreKey::new(key)).is_some_and(|s| s.info.is_some())
    }

    fn read_pack(&mut self, loc: PackLoc) -> Result<Vec<u8>, Error> {
        let f = self.pack.as_mut().ok_or(Error::SessionClosed)?;
        let mut buf = vec![0u8; loc.len as usize];
        f.seek(SeekFrom::Start(loc.offset))?;
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Reads a torrent's info dictionary from the pack.
    pub fn load_metadata(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        match self.saved.get(&StoreKey::new(key)).map(|s| s.info).filter(|l| l.is_some()) {
            Some(loc) => self.read_pack(loc).map(Some),
            None => Ok(None),
        }
    }

    /// Saves a torrent's v2 piece layers (a bencoded "piece layers" dictionary) beside its info
    /// dictionary, replacing earlier ones.
    pub fn put_piece_layers(&mut self, key: &str, layers: &[u8]) -> Result<(), Error> {
        if self.load_piece_layers(key)?.as_deref() == Some(layers) {
            return Ok(());
        }
        let loc = self.append(&format!("{key}{LAYERS_SUFFIX}"), layers)?;
        self.layers.insert(StoreKey::new(key), loc);
        Ok(())
    }

    /// A torrent's v2 piece layers, if they were saved.
    pub fn load_piece_layers(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        match self.layers.get(&StoreKey::new(key)).copied() {
            Some(loc) => self.read_pack(loc).map(Some),
            None => Ok(None),
        }
    }

    /// Appends a record to the pack; returns where its data is.
    fn append(&mut self, pack_key: &str, data: &[u8]) -> Result<PackLoc, Error> {
        let f = self.pack.as_mut().ok_or(Error::SessionClosed)?;
        let rec = pack_record(pack_key, data);
        f.seek(SeekFrom::Start(self.pack_len))?;
        f.write_all(&rec)?;
        let offset = self.pack_len + rec.len() as u64 - data.len() as u64;
        self.pack_len += rec.len() as u64;
        Ok(PackLoc { offset, len: data.len() as u32 })
    }

    /// Saves a torrent's record. `info` is appended to the pack if it is not there yet.
    pub fn put(&mut self, key: &str, record: &ResumeData, info: Option<&[u8]>) -> Result<(), Error> {
        let mut rec = record.clone();
        rec.info = None;
        let bytes = rec.encode();
        let hash = record_hash(&bytes);
        let saved =
            self.saved.entry(StoreKey::new(key)).or_insert(Saved { hash: !hash, info: PackLoc::NONE });
        if saved.hash != hash {
            saved.hash = hash;
            push_log_entry(&mut self.pending, key, Some(&bytes));
        }
        let stored = saved.info.is_some();
        if let Some(info) = info.filter(|i| !stored && !i.is_empty()) {
            let loc = self.append(key, info)?;
            if let Some(s) = self.saved.get_mut(&StoreKey::new(key)) {
                s.info = loc;
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, key: &str) {
        let key_bin = StoreKey::new(key);
        if self.saved.remove(&key_bin).is_some() {
            push_log_entry(&mut self.pending, key, None);
        }
        self.layers.remove(&key_bin);
    }

    #[cfg(test)]
    pub fn is_dirty(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Appends the changed records to the journal (and syncs it and the pack); folds the journal
    /// into `resume.db` once it has grown.
    pub fn flush(&mut self) -> Result<(), Error> {
        if self.pending.is_empty() {
            return Ok(());
        }
        {
            let mut f = OpenOptions::new().create(true).append(true).open(self.dir.join(RESUME_LOG))?;
            f.write_all(&self.pending)?;
            f.sync_data()?;
        }
        self.log_len += self.pending.len() as u64;
        // a save of every torrent (100,000 records are 87 MB) must not stay allocated
        if self.pending.capacity() > PENDING_KEEP {
            self.pending = Vec::new();
        } else {
            self.pending.clear();
        }
        if let Some(p) = &self.pack {
            p.sync_data()?;
        }
        if self.log_len >= COMPACT_LOG_MIN && self.log_len > self.db_len / 2 {
            let (db, log) = self.read_files()?;
            let records = self.read_records(db.as_deref(), &log)?;
            self.write_db(&records)?;
        }
        Ok(())
    }
}

/// The hash `put` compares records by.
fn record_hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

fn push_log_entry(out: &mut Vec<u8>, key: &str, data: Option<&[u8]>) {
    out.extend_from_slice(LOG_MAGIC);
    out.extend_from_slice(&(key.len() as u16).to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    match data {
        Some(d) => {
            out.extend_from_slice(&(d.len() as u32).to_le_bytes());
            out.extend_from_slice(d);
        }
        None => out.extend_from_slice(&REMOVED.to_le_bytes()),
    }
}

/// The records of a `resume.db` file, added to `records` by key.
pub(crate) fn parse_db<'a>(buf: &'a [u8], records: &mut BTreeMap<String, &'a [u8]>) -> Result<(), Error> {
    // our own file, one dictionary of every record: the default token limit (2 million, about
    // 22,000 torrents) guards against hostile input, not this. Every token takes at least two
    // bytes.
    let token_limit = (buf.len() / 2).max(16);
    let doc = bdecode_with_limits(buf, DEFAULT_DEPTH_LIMIT, token_limit)
        .map_err(|e| Error::InvalidResumeData(format!("{RESUME_DB}: {}", e.kind.message())))?;
    let root = doc.root().filter(|r| r.dict_find_str("file-format") == Some(DB_FORMAT));
    let root = root.ok_or_else(|| Error::InvalidResumeData(format!("{RESUME_DB}: not a resume database")))?;
    if let Some(t) = root.dict_find_dict("torrents") {
        for (key, rec) in t.dict_iter().nodes() {
            let key = String::from_utf8_lossy(key.string_value()).into_owned();
            // the record where it is in `buf` (the document's nodes borrow the document)
            let section = rec.data_section();
            let at = section.as_ptr() as usize - buf.as_ptr() as usize;
            records.insert(key, &buf[at..at + section.len()]);
        }
    }
    Ok(())
}

/// Replays the journal `log` over `records`; returns the length of its intact part (what
/// follows is a torn or foreign entry).
pub(crate) fn replay_log<'a>(log: &'a [u8], records: &mut BTreeMap<String, &'a [u8]>) -> usize {
    let mut pos = 0usize;
    while let Some((key, data, next)) = log_entry(log, pos) {
        match data {
            Some(data) => records.insert(key, data),
            None => records.remove(&key),
        };
        pos = next;
    }
    pos
}

/// The journal entry at `pos`: its key, its record (`None` for a removal) and where the next
/// one starts; `None` at the end or at a torn or foreign entry. The offsets are added with
/// overflow checks: a length near `u32::MAX` must not wrap on a 32-bit target.
fn log_entry(log: &[u8], pos: usize) -> Option<(String, Option<&[u8]>, usize)> {
    let head = log.get(pos..pos.checked_add(6)?)?;
    if &head[..4] != LOG_MAGIC {
        return None;
    }
    let klen = u16::from_le_bytes([head[4], head[5]]) as usize;
    let len_at = pos.checked_add(6 + klen)?;
    let key = log.get(pos + 6..len_at)?;
    let data_at = len_at.checked_add(4)?;
    let len = u32::from_le_bytes(log.get(len_at..data_at)?.try_into().ok()?);
    let key = String::from_utf8_lossy(key).into_owned();
    if len == REMOVED {
        return Some((key, None, data_at));
    }
    let end = data_at.checked_add(usize::try_from(len).ok()?)?;
    let data = log.get(data_at..end)?;
    Some((key, Some(data), end))
}

fn pack_record(key: &str, data: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(10 + key.len() + data.len());
    r.extend_from_slice(PACK_MAGIC);
    r.extend_from_slice(&(key.len() as u16).to_le_bytes());
    r.extend_from_slice(key.as_bytes());
    r.extend_from_slice(&(data.len() as u32).to_le_bytes());
    r.extend_from_slice(data);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{InfoHashes, Sha1Hash};

    fn rec(i: u8) -> ResumeData {
        ResumeData {
            info_hashes: InfoHashes::v1(Sha1Hash([i; 20])),
            name: format!("t{i}"),
            save_path: "/dl".into(),
            ..Default::default()
        }
    }

    /// Security review 2026-10-22: crafted journal entries (lengths past the end, a length of
    /// `u32::MAX - 1`, a position at the end of the address space) stop the replay at the last
    /// intact entry instead of wrapping an offset.
    #[test]
    fn crafted_journal_entries_end_the_replay() {
        let mut log = Vec::new();
        push_log_entry(&mut log, "a", Some(b"d1:xi1ee"));
        let intact = log.len();
        for len in [u32::MAX - 1, 1000, 9] {
            let mut bad = log.clone();
            bad.extend_from_slice(LOG_MAGIC);
            bad.extend_from_slice(&1u16.to_le_bytes());
            bad.push(b'b');
            bad.extend_from_slice(&len.to_le_bytes());
            bad.extend_from_slice(b"d1:xi2e");
            let mut records = BTreeMap::new();
            assert_eq!(replay_log(&bad, &mut records), intact, "length {len}");
            assert_eq!(records.keys().collect::<Vec<_>>(), ["a"]);
        }
        // a key length past the end
        let mut bad = log.clone();
        bad.extend_from_slice(LOG_MAGIC);
        bad.extend_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(replay_log(&bad, &mut BTreeMap::new()), intact);
        assert!(log_entry(&log, usize::MAX - 3).is_none());
        assert!(log_entry(&log, usize::MAX).is_none());
        // a removal entry
        push_log_entry(&mut log, "a", None);
        let mut records = BTreeMap::new();
        assert_eq!(replay_log(&log, &mut records), log.len());
        assert!(records.is_empty());
        // resume.db: not ours, and not bencode
        assert!(parse_db(b"d11:file-format3:fooe", &mut BTreeMap::new()).is_err());
        assert!(parse_db(b"d8:torrents", &mut BTreeMap::new()).is_err());
    }

    /// Saves append the changed records to the journal only; it is replayed over `resume.db`
    /// (updates and removals), a torn entry at its end is cut, and once it has grown it is
    /// folded into `resume.db`.
    #[test]
    fn the_journal_holds_changes_and_is_folded_in() {
        let dir = crate::disk::tests::temp_dir("store-journal");
        let names = |list: &[ResumeData]| {
            let mut n: Vec<String> = list.iter().map(|r| r.name.clone()).collect();
            n.sort();
            n
        };
        {
            let (mut s, _, _) = Store::open(&dir).unwrap();
            for i in 1..=3 {
                s.put(&format!("k{i}"), &rec(i), None).unwrap();
            }
            s.flush().unwrap();
            assert!(!dir.join(RESUME_DB).exists(), "nothing folded in yet");
            let journal = std::fs::metadata(dir.join(RESUME_LOG)).unwrap().len();
            // an unchanged record writes nothing; a changed one and a removal are appended
            s.put("k1", &rec(1), None).unwrap();
            assert!(!s.is_dirty());
            let mut changed = rec(2);
            changed.name = "renamed".into();
            s.put("k2", &changed, None).unwrap();
            s.remove("k3");
            s.flush().unwrap();
            let grown = std::fs::metadata(dir.join(RESUME_LOG)).unwrap().len();
            assert!(grown > journal && grown < 2 * journal, "only the changes: {journal} -> {grown}");
        }
        // a crash while appending
        let mut f = OpenOptions::new().append(true).open(dir.join(RESUME_LOG)).unwrap();
        f.write_all(b"LTR1\x02\x00k4\xff\xff").unwrap();
        drop(f);
        {
            let (mut s, list, errors) = Store::open(&dir).unwrap();
            assert!(errors.is_empty());
            assert_eq!(names(&list), ["renamed", "t1"]);
            // grow the journal past the folding threshold
            let mut r = rec(5);
            r.save_path = "x".repeat(1 << 16);
            for i in 0..80 {
                r.name = format!("big{i}");
                s.put("k5", &r, None).unwrap();
                s.flush().unwrap();
            }
            assert!(std::fs::metadata(dir.join(RESUME_LOG)).unwrap().len() < COMPACT_LOG_MIN);
            assert!(dir.join(RESUME_DB).exists(), "folded into resume.db");
        }
        let (_, list, _) = Store::open(&dir).unwrap();
        assert_eq!(names(&list), ["big79", "renamed", "t1"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A database with more tokens than bdecode's default limit (about 22,000 torrents) still
    /// loads: the session would otherwise start empty.
    #[test]
    fn a_resume_database_beyond_the_default_token_limit_loads() {
        let dir = crate::disk::tests::temp_dir("store-big");
        let n = 40_000;
        {
            let (mut s, _, _) = Store::open(&dir).unwrap();
            for i in 0..n {
                let mut r = rec((i % 251) as u8);
                r.name = format!("t{i}");
                s.put(&format!("{i:040x}"), &r, None).unwrap();
            }
            s.flush().unwrap();
        }
        let buf = std::fs::read(dir.join(RESUME_DB)).unwrap();
        assert!(crate::bencode::bdecode(&buf[..]).is_err(), "beyond the default token limit");
        let (_, list, errors) = Store::open(&dir).unwrap();
        assert!(errors.is_empty());
        assert_eq!(list.len(), n as usize);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A `resume.db` and journal from before file-version 2 (a byte per piece) load, and the
    /// next save writes the record over as version 2.
    #[test]
    fn version_1_records_load_and_are_saved_as_version_2() {
        let dir = crate::disk::tests::temp_dir("store-v1");
        let v1 = |i: u8, pieces: &[u8]| {
            let mut e = rec(i).to_entry();
            *e.key("file-version") = Entry::Int(1);
            *e.key("pieces") = Entry::Bytes(pieces.to_vec());
            *e.key("lt-num-pieces") = Entry::Int(pieces.len() as i64);
            e.bencode()
        };
        let mut torrents = Entry::new_dict();
        *torrents.key("a") = Entry::Preformatted(v1(1, &[1, 0, 1]));
        let mut root = Entry::new_dict();
        *root.key("file-format") = Entry::Bytes(DB_FORMAT.as_bytes().to_vec());
        *root.key("file-version") = Entry::Int(1);
        *root.key("torrents") = torrents;
        std::fs::write(dir.join(RESUME_DB), root.bencode()).unwrap();
        let mut log = Vec::new();
        push_log_entry(&mut log, "b", Some(&v1(2, &[3; 9])));
        std::fs::write(dir.join(RESUME_LOG), log).unwrap();
        let pieces = |list: &[ResumeData], name: &str| -> Vec<bool> {
            let r = list.iter().find(|r| r.name == name).unwrap();
            r.pieces.as_ref().unwrap().iter().collect()
        };
        {
            let (mut s, list, errors) = Store::open(&dir).unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(pieces(&list, "t1"), [true, false, true]);
            assert_eq!(pieces(&list, "t2"), [true; 9]);
            for (key, r) in [("a", &list[0]), ("b", &list[1])] {
                s.put(key, r, None).unwrap();
            }
            assert!(s.is_dirty(), "rewritten as version 2");
            s.flush().unwrap();
        }
        let (_, list, errors) = Store::open(&dir).unwrap();
        assert!(errors.is_empty());
        assert_eq!(pieces(&list, "t1"), [true, false, true]);
        assert_eq!(pieces(&list, "t2"), [true; 9]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_reload_remove_and_recover() {
        let dir = crate::disk::tests::temp_dir("store");
        {
            let (mut s, list, errors) = Store::open(&dir).unwrap();
            assert!(list.is_empty() && errors.is_empty());
            s.put("a", &rec(1), Some(b"d4:name1:ae")).unwrap();
            s.put("b", &rec(2), Some(b"d4:name1:be")).unwrap();
            s.put("b", &rec(2), Some(b"ignored, already stored")).unwrap();
            assert!(s.is_dirty());
            s.flush().unwrap();
            assert!(!s.is_dirty());
            s.remove("a");
            s.flush().unwrap();
        }
        // Simulate a crash in the middle of appending a record.
        let mut f = OpenOptions::new().append(true).open(dir.join(METADATA_PACK)).unwrap();
        f.write_all(b"LTI1\x05\x00abc").unwrap();
        drop(f);
        {
            let (mut s, list, _) = Store::open(&dir).unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].name, "t2");
            assert!(!s.has_metadata("a"), "removed torrents are dead in the pack");
            assert_eq!(s.load_metadata("b").unwrap().unwrap(), b"d4:name1:be");
            s.put("c", &rec(3), Some(b"d4:name1:ce")).unwrap();
            s.flush().unwrap();
        }
        let (mut s, list, _) = Store::open(&dir).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            s.load_metadata("c").unwrap().unwrap(),
            b"d4:name1:ce",
            "appended after the torn record was cut"
        );
        s.compact().unwrap();
        assert_eq!(s.load_metadata("b").unwrap().unwrap(), b"d4:name1:be");
        assert_eq!(s.load_metadata("c").unwrap().unwrap(), b"d4:name1:ce");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Keys are held in binary when they are info-hash hex (v1 or v2) and as text otherwise;
    /// either way they come back as written, and a torrent's info dictionary and piece layers
    /// live and die with its record.
    #[test]
    fn keys_are_held_in_binary_and_round_trip() {
        let v1 = "0123456789abcdef0123456789abcdef01234567";
        let v2 = "ab".repeat(32);
        for k in [v1, v2.as_str(), "k1", "0123456789ABCDEF0123456789ABCDEF01234567", "zz"] {
            assert_eq!(StoreKey::new(k).to_string(), k);
        }
        assert!(matches!(StoreKey::new(v1), StoreKey::V1(_)));
        assert!(matches!(StoreKey::new(&v2), StoreKey::V2(_)));
        assert!(matches!(StoreKey::new(&v1.to_uppercase()), StoreKey::Other(_)), "not our spelling");

        let dir = crate::disk::tests::temp_dir("store-keys");
        {
            let (mut s, _, _) = Store::open(&dir).unwrap();
            s.put(v1, &rec(1), Some(b"d4:name1:ae")).unwrap();
            s.put(&v2, &rec(2), Some(b"d4:name1:be")).unwrap();
            s.put_piece_layers(&v2, b"de").unwrap();
            s.put("odd", &rec(3), None).unwrap();
            s.flush().unwrap();
        }
        {
            let (mut s, list, errors) = Store::open(&dir).unwrap();
            assert_eq!((list.len(), errors.len()), (3, 0));
            assert!(s.has_metadata(v1) && s.has_metadata(&v2) && !s.has_metadata("odd"));
            assert_eq!(s.load_metadata(&v2).unwrap().unwrap(), b"d4:name1:be");
            assert_eq!(s.load_piece_layers(&v2).unwrap().unwrap(), b"de");
            assert_eq!(s.load_piece_layers(v1).unwrap(), None);
            s.remove(&v2);
            s.flush().unwrap();
            assert!(!s.has_metadata(&v2) && s.load_piece_layers(&v2).unwrap().is_none());
        }
        let (mut s, list, _) = Store::open(&dir).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(s.load_metadata(v1).unwrap().unwrap(), b"d4:name1:ae");
        assert!(!s.has_metadata(&v2), "a removed torrent's pack records are dead");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
