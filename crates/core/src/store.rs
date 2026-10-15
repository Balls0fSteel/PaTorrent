//! Session persistence (ARCHITECTURE.md "Persistence", PLAN.md D5): one resume database and one
//! metadata pack per session directory, instead of two files per torrent.
//!
//! * `resume.db`: a bencoded dictionary `{"file-format", "file-version", "torrents": {key:
//!   record}}`, where each record is a libtorrent fast-resume dictionary without the `info` key.
//!   It is rewritten atomically (temporary file + rename) when something changed.
//! * `metadata.pack`: append-only info dictionaries. Each record is `"LTI1"`, a little-endian
//!   `u16` key length, the key (info-hash hex), a little-endian `u32` length and the bencoded
//!   info dictionary. Records are read only when a torrent starts, so stopped torrents never
//!   hold their piece hashes in memory. The pack is compacted when more than half of it is
//!   dead.
//!
//! New code (libtorrent stores resume data through the client).

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::bencode::{Entry, bdecode};
use crate::error::Error;
use crate::resume::ResumeData;

const RESUME_DB: &str = "resume.db";
const METADATA_PACK: &str = "metadata.pack";
const PACK_MAGIC: &[u8; 4] = b"LTI1";
const DB_FORMAT: &str = "patorrent resume db";

/// Location of an info dictionary in the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackLoc {
    offset: u64,
    len: u32,
}

pub(crate) struct Store {
    dir: PathBuf,
    /// Encoded records, by key.
    records: BTreeMap<String, Vec<u8>>,
    dirty: bool,
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
            records: BTreeMap::new(),
            dirty: false,
            pack: None,
            pack_index: HashMap::new(),
            pack_len: 0,
        };
        let mut torrents = Vec::new();
        let mut errors = Vec::new();
        match std::fs::read(dir.join(RESUME_DB)) {
            Ok(buf) => {
                let doc = bdecode(&buf[..])
                    .map_err(|e| Error::InvalidResumeData(format!("{RESUME_DB}: {}", e.kind.message())))?;
                let root = doc.root().filter(|r| r.dict_find_str("file-format") == Some(DB_FORMAT));
                let root = root
                    .ok_or_else(|| Error::InvalidResumeData(format!("{RESUME_DB}: not a resume database")))?;
                if let Some(t) = root.dict_find_dict("torrents") {
                    for (key, rec) in t.dict_iter().nodes() {
                        let key = String::from_utf8_lossy(key.string_value()).into_owned();
                        match ResumeData::from_node(rec) {
                            Ok(r) => {
                                store.records.insert(key, rec.data_section().to_vec());
                                torrents.push(r);
                            }
                            Err(e) => errors.push(format!("{key}: {e}")),
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        store.open_pack()?;
        let live = store.pack_index.values().map(|l| l.len as u64 + 64).sum::<u64>();
        if store.pack_len > 1024 * 1024 && store.pack_len > 2 * live {
            store.compact()?;
        }
        Ok((store, torrents, errors))
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
            if self.records.contains_key(&key) {
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

    /// Saves a torrent's record. `info` is appended to the pack if it is not there yet.
    pub fn put(&mut self, key: &str, record: &ResumeData, info: Option<&[u8]>) -> Result<(), Error> {
        let mut rec = record.clone();
        rec.info = None;
        let bytes = rec.encode();
        if self.records.get(key) != Some(&bytes) {
            self.records.insert(key.to_owned(), bytes);
            self.dirty = true;
        }
        if let Some(info) = info {
            if !self.pack_index.contains_key(key) {
                let f = self.pack.as_mut().ok_or(Error::SessionClosed)?;
                let rec = pack_record(key, info);
                f.seek(SeekFrom::Start(self.pack_len))?;
                f.write_all(&rec)?;
                let offset = self.pack_len + rec.len() as u64 - info.len() as u64;
                self.pack_len += rec.len() as u64;
                self.pack_index.insert(key.to_owned(), PackLoc { offset, len: info.len() as u32 });
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, key: &str) {
        if self.records.remove(key).is_some() {
            self.dirty = true;
        }
        self.pack_index.remove(key);
    }

    #[cfg(test)]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Writes `resume.db` if anything changed (atomically: temporary file, flush, rename).
    pub fn flush(&mut self) -> Result<(), Error> {
        if !self.dirty {
            return Ok(());
        }
        let mut torrents = Entry::new_dict();
        for (k, v) in &self.records {
            *torrents.key(k) = Entry::Preformatted(v.clone());
        }
        let mut root = Entry::new_dict();
        *root.key("file-format") = Entry::Bytes(DB_FORMAT.as_bytes().to_vec());
        *root.key("file-version") = Entry::Int(1);
        *root.key("torrents") = torrents;
        let tmp = self.dir.join(format!("{RESUME_DB}.tmp"));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&root.bencode())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.dir.join(RESUME_DB))?;
        if let Some(p) = &self.pack {
            p.sync_data()?;
        }
        self.dirty = false;
        Ok(())
    }
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
