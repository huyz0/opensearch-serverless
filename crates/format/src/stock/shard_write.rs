//! The two shard-level blobs an export writes: a snapshot's own list of a
//! shard's files (`snap-<uuid>.dat`) and the shard's generation
//! (`index-<generation>`), which lists every file and which snapshot holds
//! which. The JSON is laid out as a stock node writes it; each result goes
//! through [`super::from_json`] into the envelope.
//!
//! Every file is a whole blob (`part_size` is `Long.MAX_VALUE`, as stock
//! writes it for an unsplit file) and none is stored inline.

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};
use crate::json_write::{push_string, push_u64};

use super::repository_data::is_blob_name;
use super::shard_snapshot::MAX_FILES;

const WHAT: &str = "stock shard blob";

/// `Long.MAX_VALUE`: the part size of a file that is not split.
const UNSPLIT: u64 = i64::MAX as u64;
/// The most entries read across one generation blob.
const MAX_ENTRIES: usize = 1_000_000;

/// A file of the shard's commit, copied as one blob.
#[derive(Debug, Clone, Copy)]
pub struct NewFile<'a> {
    /// The blob it is stored under (`__<id>`).
    pub name: &'a str,
    /// The Lucene file name it is restored as.
    pub physical_name: &'a str,
    /// Its length in bytes.
    pub length: u64,
    /// Its footer CRC as [`checksum_string`] writes it.
    pub checksum: &'a str,
    /// The Lucene version that wrote it.
    pub written_by: &'a str,
}

/// A Lucene checksum as a stock file records it: the CRC32 in base 36, as
/// `Long.toString(crc, 36)`.
#[must_use]
pub fn checksum_string(crc: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let (mut n, mut out) = (u64::from(crc), Vec::new()); // ALLOC: at most 7 bytes.
    loop {
        let d = usize::try_from(n % 36).unwrap_or(0);
        out.push(DIGITS.get(d).copied().unwrap_or(b'0'));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default() // ALLOC: one short string.
}

fn check_file(f: &NewFile<'_>) -> Result<(), FormatError> {
    let base36 = !f.checksum.is_empty()
        && f.checksum.len() <= 13
        && f.checksum
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
    if !is_blob_name(f.name) || !is_blob_name(f.physical_name) || !is_blob_name(f.written_by) {
        return Err(corrupt(WHAT, "a file name that is not safe in a blob key"));
    }
    // A data blob is `__<id>`: any other name could land on the generation
    // or the snapshot blob beside it, and `v__` names a file with no blob.
    if !f.name.starts_with("__") {
        return Err(corrupt(WHAT, "a data blob not named `__<id>`"));
    }
    if !base36 {
        return Err(corrupt(WHAT, "a checksum that is not base 36"));
    }
    Ok(())
}

fn check_files(files: &[NewFile<'_>]) -> Result<(), FormatError> {
    if files.is_empty() || files.len() > MAX_FILES {
        return Err(corrupt(WHAT, "a shard snapshot of no files or too many"));
    }
    files.iter().try_for_each(check_file)?;
    // ALLOC: one entry a file, at most MAX_FILES.
    let mut seen = std::collections::HashSet::new();
    if files.iter().any(|f| !seen.insert(f.name)) {
        return Err(corrupt(WHAT, "a blob named twice"));
    }
    // ALLOC: one entry a file, at most MAX_FILES.
    let mut physical = std::collections::HashSet::new();
    if files.iter().any(|f| !physical.insert(f.physical_name)) {
        return Err(corrupt(WHAT, "a Lucene file named twice"));
    }
    Ok(())
}

fn push_file(out: &mut Vec<u8>, f: &NewFile<'_>) {
    out.extend_from_slice(b"{\"name\":");
    push_string(out, f.name);
    out.extend_from_slice(b",\"physical_name\":");
    push_string(out, f.physical_name);
    out.extend_from_slice(b",\"length\":");
    push_u64(out, f.length);
    out.extend_from_slice(b",\"checksum\":");
    push_string(out, f.checksum);
    out.extend_from_slice(b",\"part_size\":");
    push_u64(out, UNSPLIT);
    out.extend_from_slice(b",\"written_by\":");
    push_string(out, f.written_by);
    out.push(b'}');
}

/// The JSON of a shard snapshot blob: the files one snapshot holds of one
/// shard, appended to `out`; a refusal leaves `out` as it was.
pub fn shard_snapshot_json(
    name: &str,
    index_version: u64,
    start_time_ms: u64,
    time_ms: u64,
    files: &[NewFile<'_>],
    out: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if name.is_empty() {
        return Err(corrupt(WHAT, "a shard snapshot without a name"));
    }
    check_files(files)?;
    let total = files.iter().fold(0u64, |n, f| n.saturating_add(f.length));
    out.extend_from_slice(b"{\"name\":");
    push_string(out, name);
    out.extend_from_slice(b",\"index_version\":");
    push_u64(out, index_version);
    out.extend_from_slice(b",\"start_time\":");
    push_u64(out, start_time_ms);
    out.extend_from_slice(b",\"time\":");
    push_u64(out, time_ms);
    out.extend_from_slice(b",\"number_of_files\":");
    push_u64(out, u64::try_from(files.len()).unwrap_or(u64::MAX));
    out.extend_from_slice(b",\"total_size\":");
    push_u64(out, total);
    out.extend_from_slice(b",\"files\":[");
    for (n, f) in files.iter().enumerate() {
        if n > 0 {
            out.push(b',');
        }
        push_file(out, f);
    }
    out.extend_from_slice(b"]}");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GenFile {
    name: String,
    physical_name: String,
    length: u64,
    checksum: String,
    part_size: u64,
    written_by: Option<String>,
    meta_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GenSnapshot {
    name: String,
    files: Vec<String>,
    shard_state_id: Option<String>,
}

/// A shard's generation blob: every file the shard's snapshots hold and
/// which snapshot holds which.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShardGeneration {
    files: Vec<GenFile>,
    snapshots: Vec<GenSnapshot>,
}

impl ShardGeneration {
    /// Reads the JSON of a generation blob.
    pub fn parse(json: &[u8]) -> Result<Self, FormatError> {
        Self::parse_with(json, MAX_ENTRIES)
    }

    /// Reads `json`, refusing more than `max_entries` entries in all.
    pub fn parse_with(json: &[u8], max_entries: usize) -> Result<Self, FormatError> {
        let mut r = Reader::new(json, WHAT);
        r.start_object("a shard generation is an object")?;
        let mut out = Self::default();
        let mut left = max_entries;
        while let Some(field) = r.next_field()? {
            match field.as_ref() {
                "files" => out.files = gen_files(&mut r, &mut left)?,
                "snapshots" => out.snapshots = gen_snapshots(&mut r, &mut left)?,
                _ => r.skip_value()?,
            }
        }
        r.finish()?;
        // ALLOC: one entry a file and a snapshot, bounded by the entry budget.
        let mut files = std::collections::HashSet::new();
        let mut snaps = std::collections::HashSet::new();
        if out.files.iter().any(|f| !files.insert(f.name.as_str()))
            || out.snapshots.iter().any(|s| !snaps.insert(s.name.as_str()))
        {
            return Err(corrupt(WHAT, "a file or a snapshot listed twice"));
        }
        Ok(out)
    }

    /// The names of the snapshots it holds, in file order.
    #[must_use]
    pub fn snapshots(&self) -> Vec<&str> {
        // ALLOC: one entry a snapshot of the blob.
        self.snapshots.iter().map(|s| s.name.as_str()).collect()
    }

    /// The blobs `snapshot` holds, when it is held.
    #[must_use]
    pub fn files_of(&self, snapshot: &str) -> Option<&[String]> {
        self.snapshots
            .iter()
            .find(|s| s.name == snapshot)
            .map(|s| s.files.as_slice())
    }

    /// How many files the blob lists.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// This generation with snapshot `name` added, holding `files`, each a
    /// new blob. Every earlier file and snapshot is kept as read.
    pub fn with_snapshot(&self, name: &str, files: &[NewFile<'_>]) -> Result<Self, FormatError> {
        self.with_snapshot_within(name, files, MAX_ENTRIES)
    }

    /// [`Self::with_snapshot`], refusing a result of more than `max_entries`
    /// entries: the bound [`Self::parse`] holds, so the blob written can be read.
    pub fn with_snapshot_within(
        &self,
        name: &str,
        files: &[NewFile<'_>],
        max_entries: usize,
    ) -> Result<Self, FormatError> {
        if name.is_empty() || self.snapshots.iter().any(|s| s.name == name) {
            return Err(corrupt(WHAT, "a snapshot with no name or one already held"));
        }
        check_files(files)?;
        // ALLOC: one entry a held file, bounded by the entry budget.
        let held: std::collections::HashSet<&str> =
            self.files.iter().map(|f| f.name.as_str()).collect();
        if files.iter().any(|f| held.contains(f.name)) {
            return Err(corrupt(WHAT, "a blob already listed"));
        }
        let refs = self
            .snapshots
            .iter()
            .fold(0usize, |n, s| n.saturating_add(s.files.len()));
        let entries = self
            .files
            .len()
            .saturating_add(self.snapshots.len())
            .saturating_add(refs)
            .saturating_add(files.len().saturating_mul(2))
            .saturating_add(1);
        if entries > max_entries {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: u64::try_from(max_entries).unwrap_or(u64::MAX),
                actual: u64::try_from(entries).unwrap_or(u64::MAX),
            });
        }
        // ALLOC: a copy of the generation, bounded by its entries.
        let mut next = self.clone();
        for f in files {
            next.files.push(GenFile {
                name: f.name.to_owned(),                   // ALLOC: one name a file.
                physical_name: f.physical_name.to_owned(), // ALLOC: one name a file.
                length: f.length,
                checksum: f.checksum.to_owned(), // ALLOC: one checksum a file.
                part_size: UNSPLIT,
                written_by: Some(f.written_by.to_owned()), // ALLOC: one version a file.
                meta_hash: None,
            });
        }
        next.snapshots.push(GenSnapshot {
            name: name.to_owned(),                                    // ALLOC: one name.
            files: files.iter().map(|f| f.name.to_owned()).collect(), // ALLOC: one name a file.
            shard_state_id: None,
        });
        Ok(next)
    }

    /// The generation's JSON, appended to `out`.
    pub fn to_json(&self, out: &mut Vec<u8>) -> Result<(), FormatError> {
        out.extend_from_slice(b"{\"files\":[");
        for (n, f) in self.files.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            out.extend_from_slice(b"{\"name\":");
            push_string(out, &f.name);
            out.extend_from_slice(b",\"physical_name\":");
            push_string(out, &f.physical_name);
            out.extend_from_slice(b",\"length\":");
            push_u64(out, f.length);
            out.extend_from_slice(b",\"checksum\":");
            push_string(out, &f.checksum);
            out.extend_from_slice(b",\"part_size\":");
            push_u64(out, f.part_size);
            if let Some(w) = &f.written_by {
                out.extend_from_slice(b",\"written_by\":");
                push_string(out, w);
            }
            if let Some(h) = &f.meta_hash {
                out.extend_from_slice(b",\"meta_hash\":");
                push_string(out, h);
            }
            out.push(b'}');
        }
        out.extend_from_slice(b"],\"snapshots\":{");
        for (n, s) in self.snapshots.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            push_string(out, &s.name);
            out.extend_from_slice(b":{\"files\":[");
            for (m, f) in s.files.iter().enumerate() {
                if m > 0 {
                    out.push(b',');
                }
                push_string(out, f);
            }
            out.push(b']');
            if let Some(id) = &s.shard_state_id {
                out.extend_from_slice(b",\"shard_state_id\":");
                push_string(out, id);
            }
            out.push(b'}');
        }
        out.extend_from_slice(b"}}");
        Ok(())
    }
}

fn spend(left: &mut usize, r: &Reader<'_>) -> Result<(), FormatError> {
    *left = left
        .checked_sub(1)
        .ok_or_else(|| r.error("more entries than a reader accepts"))?;
    Ok(())
}

fn string(r: &mut Reader<'_>, key: bool) -> Result<String, FormatError> {
    match r.expect_token()? {
        Token::Str(s) if !key || is_blob_name(&s) => Ok(s.into_owned()), // ALLOC: one string.
        _ => Err(r.error("expected a string, safe in a blob key where it is one")),
    }
}

fn unsigned(r: &mut Reader<'_>) -> Result<u64, FormatError> {
    match r.expect_token()? {
        Token::Int(n) => u64::try_from(n).map_err(|_| r.error("a negative number")),
        _ => Err(r.error("expected an integer")),
    }
}

fn gen_files(r: &mut Reader<'_>, left: &mut usize) -> Result<Vec<GenFile>, FormatError> {
    match r.expect_token()? {
        Token::StartArray => {}
        _ => return Err(r.error("files is an array")),
    }
    let mut out = Vec::new(); // ALLOC: one entry a file, bounded by the entry budget.
    loop {
        match r.expect_token()? {
            Token::EndArray => return Ok(out),
            Token::StartObject => {}
            _ => return Err(r.error("a file is an object")),
        }
        spend(left, r)?;
        let (mut name, mut physical, mut length, mut checksum) = (None, None, None, None);
        let (mut part_size, mut written_by, mut meta_hash) = (UNSPLIT, None, None);
        while let Some(field) = r.next_field()? {
            match field.as_ref() {
                "name" => name = Some(string(r, true)?),
                "physical_name" => physical = Some(string(r, true)?),
                "checksum" => checksum = Some(string(r, false)?),
                "written_by" => written_by = Some(string(r, false)?),
                "meta_hash" => meta_hash = Some(string(r, false)?),
                "length" => length = Some(unsigned(r)?),
                "part_size" => part_size = unsigned(r)?,
                _ => r.skip_value()?,
            }
        }
        let (Some(name), Some(physical_name), Some(length), Some(checksum)) =
            (name, physical, length, checksum)
        else {
            return Err(r.error("a file without a name, length or checksum"));
        };
        out.push(GenFile {
            name,
            physical_name,
            length,
            checksum,
            part_size,
            written_by,
            meta_hash,
        });
    }
}

fn gen_snapshots(r: &mut Reader<'_>, left: &mut usize) -> Result<Vec<GenSnapshot>, FormatError> {
    r.start_object("snapshots is an object")?;
    let mut out = Vec::new(); // ALLOC: one entry a snapshot, bounded by the entry budget.
    while let Some(name) = r.next_field()? {
        spend(left, r)?;
        let name = name.into_owned(); // ALLOC: one name a snapshot.
        r.start_object("a snapshot entry is an object")?;
        let (mut files, mut shard_state_id) = (Vec::new(), None); // ALLOC: one name a file.
        while let Some(field) = r.next_field()? {
            match field.as_ref() {
                "files" => {
                    match r.expect_token()? {
                        Token::StartArray => {}
                        _ => return Err(r.error("files is an array")),
                    }
                    loop {
                        match r.expect_token()? {
                            Token::EndArray => break,
                            Token::Str(f) if is_blob_name(&f) => {
                                spend(left, r)?;
                                files.push(f.into_owned()); // ALLOC: one name a file.
                            }
                            _ => return Err(r.error("a file name is a string, safe in a blob key")),
                        }
                    }
                }
                "shard_state_id" => shard_state_id = Some(string(r, false)?),
                _ => r.skip_value()?,
            }
        }
        out.push(GenSnapshot {
            name,
            files,
            shard_state_id,
        });
    }
    Ok(out)
}
