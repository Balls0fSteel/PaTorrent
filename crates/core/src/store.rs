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

/// Location of an info dictionary in the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackLoc {
    offset: u64,
    len: u32,
}

pub(crate) struct Store {
    dir: PathBuf,
    /// A hash of each torrent's encoded record, by key (unchanged records are not written).
    records: HashMap<String, u64>,
    /// Journal entries not written yet.
    pending: Vec<u8>,
    /// Bytes of `resume.log` and of `resume.db`.
    log_len: u64,
    db_len: u64,
    pack: Option<File>,
    pack_index: HashMap<String, PackLoc>,
    pack_len: u64,
}

impl Store {
    /// Opens (or creates) the store in `dir` and returns every saved torrent, in no particular
    /// order. Unreadable records are skipped and reported in the second list.
    pub fn open(dir: &Path) -> Result<(Store, Vec<ResumeData>, Vec<String>), Error> {
        std::fs::create_dir_all(dir)?;
        let mut store = Store {
            dir: dir.to_owned(),
            records: HashMap::new(),
            pending: Vec::new(),
            log_len: 0,
            db_len: 0,
            pack: None,
            pack_index: HashMap::new(),
            pack_len: 0,
        };
        let mut torrents = Vec::new();
        let mut errors = Vec::new();
        let records = store.read_records()?;
        for (key, bytes) in &records {
            let decoded = bdecode(&bytes[..]).map_err(|e| e.kind.message().to_owned()).and_then(|doc| {
                match doc.root() {
                    Some(root) => ResumeData::from_node(root).map_err(|e| e.to_string()),
                    None => Err("empty record".to_owned()),
                }
            });
            match decoded {
                Ok(r) => {
                    store.records.insert(key.clone(), record_hash(bytes));
                    torrents.push(r);
                }
                Err(e) => errors.push(format!("{key}: {e}")),
            }
        }
        if store.log_len >= COMPACT_LOG_MIN && store.log_len > store.db_len / 2 {
            store.write_db(&records)?;
        }
        drop(records);
        store.open_pack()?;
        let live = store.pack_index.values().map(|l| l.len as u64 + 64).sum::<u64>();
        if store.pack_len > 1024 * 1024 && store.pack_len > 2 * live {
            store.compact()?;
        }
        Ok((store, torrents, errors))
    }

    /// Every saved record: `resume.db` with `resume.log` replayed over it. A torn journal entry
    /// (a crash while appending) is cut off.
    fn read_records(&mut self) -> Result<BTreeMap<String, Vec<u8>>, Error> {
        let mut records = BTreeMap::new();
        match std::fs::read(self.dir.join(RESUME_DB)) {
            Ok(buf) => {
                self.db_len = buf.len() as u64;
                // our own file, one dictionary of every record: the default token limit (2
                // million, about 22,000 torrents) guards against hostile input, not this. Every
                // token takes at least two bytes.
                let token_limit = (buf.len() / 2).max(16);
                let doc = bdecode_with_limits(&buf[..], DEFAULT_DEPTH_LIMIT, token_limit)
                    .map_err(|e| Error::InvalidResumeData(format!("{RESUME_DB}: {}", e.kind.message())))?;
                let root = doc.root().filter(|r| r.dict_find_str("file-format") == Some(DB_FORMAT));
                let root = root
                    .ok_or_else(|| Error::InvalidResumeData(format!("{RESUME_DB}: not a resume database")))?;
                if let Some(t) = root.dict_find_dict("torrents") {
                    for (key, rec) in t.dict_iter().nodes() {
                        let key = String::from_utf8_lossy(key.string_value()).into_owned();
                        records.insert(key, rec.data_section().to_vec());
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let log_path = self.dir.join(RESUME_LOG);
        let log = match std::fs::read(&log_path) {
            Ok(log) => log,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let mut pos = 0usize;
        while let Some((key, data, next)) = log_entry(&log, pos) {
            match data {
                Some(data) => records.insert(key, data.to_vec()),
                None => records.remove(&key),
            };
            pos = next;
        }
        if pos < log.len() {
            OpenOptions::new().write(true).open(&log_path)?.set_len(pos as u64)?;
        }
        self.log_len = pos as u64;
        Ok(records)
    }

    /// Writes `records` as the new `resume.db` (atomically: temporary file, flush, rename),
    /// then empties the journal they include.
    fn write_db(&mut self, records: &BTreeMap<String, Vec<u8>>) -> Result<(), Error> {
        let mut torrents = Entry::new_dict();
        // records that did not load are left out, as they were never kept
        for (k, v) in records.iter().filter(|(k, _)| self.records.contains_key(*k)) {
            *torrents.key(k) = Entry::Preformatted(v.clone());
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
            let key = String::from_utf8_lossy(&key).into_owned();
            // Later records win; only keys with a resume record are live.
            if self.records.contains_key(key.strip_suffix(LAYERS_SUFFIX).unwrap_or(&key)) {
                self.pack_index.insert(key, PackLoc { offset: data_off, len: dlen });
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
        let keys: Vec<String> = self.pack_index.keys().cloned().collect();
        for k in keys {
            if let Some(d) = self.load_metadata(&k)? {
                entries.push((k, d));
            }
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
        self.pack_index.clear();
        self.open_pack()?;
        // Records were rewritten for live keys only; reindex them.
        Ok(())
    }

    pub fn has_metadata(&self, key: &str) -> bool {
        self.pack_index.contains_key(key)
    }

    /// Reads a torrent's info dictionary from the pack.
    pub fn load_metadata(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        let Some(loc) = self.pack_index.get(key).copied() else { return Ok(None) };
        let f = self.pack.as_mut().ok_or(Error::SessionClosed)?;
        let mut buf = vec![0u8; loc.len as usize];
        f.seek(SeekFrom::Start(loc.offset))?;
        f.read_exact(&mut buf)?;
        Ok(Some(buf))
    }

    /// Saves a torrent's v2 piece layers (a bencoded "piece layers" dictionary) beside its info
    /// dictionary, replacing earlier ones.
    pub fn put_piece_layers(&mut self, key: &str, layers: &[u8]) -> Result<(), Error> {
        let pack_key = format!("{key}{LAYERS_SUFFIX}");
        if self.load_metadata(&pack_key)?.as_deref() == Some(layers) {
            return Ok(());
        }
        self.append(&pack_key, layers)
    }

    /// A torrent's v2 piece layers, if they were saved.
    pub fn load_piece_layers(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        self.load_metadata(&format!("{key}{LAYERS_SUFFIX}"))
    }

    fn append(&mut self, pack_key: &str, data: &[u8]) -> Result<(), Error> {
        let f = self.pack.as_mut().ok_or(Error::SessionClosed)?;
        let rec = pack_record(pack_key, data);
        f.seek(SeekFrom::Start(self.pack_len))?;
        f.write_all(&rec)?;
        let offset = self.pack_len + rec.len() as u64 - data.len() as u64;
        self.pack_len += rec.len() as u64;
        self.pack_index.insert(pack_key.to_owned(), PackLoc { offset, len: data.len() as u32 });
        Ok(())
    }

    /// Saves a torrent's record. `info` is appended to the pack if it is not there yet.
    pub fn put(&mut self, key: &str, record: &ResumeData, info: Option<&[u8]>) -> Result<(), Error> {
        let mut rec = record.clone();
        rec.info = None;
        let bytes = rec.encode();
        let hash = record_hash(&bytes);
        if self.records.get(key) != Some(&hash) {
            self.records.insert(key.to_owned(), hash);
            push_log_entry(&mut self.pending, key, Some(&bytes));
        }
        if let Some(info) = info {
            if !self.pack_index.contains_key(key) {
                self.append(key, info)?;
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, key: &str) {
        if self.records.remove(key).is_some() {
            push_log_entry(&mut self.pending, key, None);
        }
        self.pack_index.remove(key);
        self.pack_index.remove(&format!("{key}{LAYERS_SUFFIX}"));
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
        self.pending.clear();
        if let Some(p) = &self.pack {
            p.sync_data()?;
        }
        if self.log_len >= COMPACT_LOG_MIN && self.log_len > self.db_len / 2 {
            let records = self.read_records()?;
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

/// The journal entry at `pos`: its key, its record (`None` for a removal) and where the next
/// one starts; `None` at the end or at a torn or foreign entry.
fn log_entry(log: &[u8], pos: usize) -> Option<(String, Option<&[u8]>, usize)> {
    let head = log.get(pos..pos + 6)?;
    if &head[..4] != LOG_MAGIC {
        return None;
    }
    let klen = u16::from_le_bytes([head[4], head[5]]) as usize;
    let key = log.get(pos + 6..pos + 6 + klen)?;
    let len_at = pos + 6 + klen;
    let len = u32::from_le_bytes(log.get(len_at..len_at + 4)?.try_into().expect("4 bytes"));
    let key = String::from_utf8_lossy(key).into_owned();
    if len == REMOVED {
        return Some((key, None, len_at + 4));
    }
    let data = log.get(len_at + 4..len_at + 4 + len as usize)?;
    Some((key, Some(data), len_at + 4 + len as usize))
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
}
