//! Writing a snapshot record and its name register's value, as the shell's
//! `SnapshotRecord.toBytes` and `SnapshotStore.encode` do, so a Java node
//! reads what a Rust node took.
//!
//! The Java writes an index's entries in a map's order, which is not
//! specified, so what is compared with its output is the parsed content.

use crate::error::{FormatError, corrupt, len64};
use crate::json::out::Out;
use crate::snapshot_aliases::{self, AliasCapture};
use crate::snapshot_global_state::GlobalState;
use crate::snapshot_state::SnapshotState;

const WHAT: &str = "snapshot record";
const MAX_COUNT_BYTES: usize = 5;
const STATE: &str = "snapshot state";

/// One shard's captured commit: the term and where each file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardCapture {
    /// The shard's number.
    pub shard: u32,
    /// The term of the writer that published the commit.
    pub term: u64,
    /// Each file's name and its location text.
    pub files: Vec<(String, String)>,
    /// Each file's name and length; omitted from the record when empty.
    pub lengths: Vec<(String, u64)>,
}

/// One index a snapshot names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexCapture {
    /// The index's name.
    pub name: String,
    /// Its uuid.
    pub uuid: String,
    /// The shard count the record states.
    pub number_of_shards: u32,
    /// Its mapping as the JSON text the Java stores, when it has one.
    pub mapping: Option<String>,
    /// Its settings as a JSON object; absent or `{}` writes nothing.
    pub settings: Option<Vec<u8>>,
    /// The shards captured, none while the take reserves the name.
    pub shards: Vec<ShardCapture>,
    /// The shards that had no commit, ascending; written only when not empty.
    pub failed_shards: Vec<u32>,
    /// The aliases the index was in, at most `MAX_ALIASES`.
    pub aliases: Vec<AliasCapture>,
    /// Whether the index was in more aliases than the record holds.
    pub aliases_truncated: bool,
}

/// A snapshot record to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDraft {
    /// The repository.
    pub repo: String,
    /// The snapshot's name.
    pub name: String,
    /// The snapshot's uuid.
    pub uuid: String,
    /// The lifetime its copied data lives under, when it has one.
    pub storage_generation: Option<String>,
    /// Whether it references the live blobs instead of copying them.
    pub shallow: bool,
    /// When it started, in milliseconds since the epoch.
    pub start_millis: i64,
    /// When it ended, in milliseconds since the epoch.
    pub end_millis: i64,
    /// The global state it captured, written only when not empty (ADR 0068).
    pub global_state: Option<GlobalState>,
    /// The indices it names.
    pub indices: Vec<IndexCapture>,
}

impl SnapshotDraft {
    /// Writes the record into `buf` and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is too short; `actual` is the
    /// length the record needs.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field("repo", &self.repo);
        out.string_field("name", &self.name);
        out.string_field("uuid", &self.uuid);
        if let Some(generation) = &self.storage_generation {
            out.string_field("storage_generation", generation);
        }
        out.bool_field("shallow", self.shallow);
        out.int_field("start_time_in_millis", self.start_millis);
        out.int_field("end_time_in_millis", self.end_millis);
        if let Some(state) = self.global_state.as_ref().filter(|s| !s.is_empty()) {
            state.write(&mut out);
        }
        out.start_object_field("indices");
        for index in &self.indices {
            index.write(&mut out);
        }
        out.end_object();
        out.end_object();
        out.finish(WHAT)
    }
}

impl IndexCapture {
    fn write(&self, out: &mut Out<'_>) {
        out.start_object_field(&self.name);
        out.string_field("uuid", &self.uuid);
        out.int_field("number_of_shards", i64::from(self.number_of_shards));
        if let Some(mapping) = &self.mapping {
            out.string_field("mapping", mapping);
        }
        if let Some(settings) = self.settings.as_deref().filter(|s| *s != b"{}") {
            out.raw_field("settings", settings);
        }
        snapshot_aliases::write(out, &self.aliases, self.aliases_truncated);
        out.start_array_field("shards");
        for shard in &self.shards {
            out.start_object();
            out.int_field("shard", i64::from(shard.shard));
            out.int_field("term", i64::try_from(shard.term).unwrap_or(i64::MAX));
            out.start_object_field("files");
            for (file, location) in &shard.files {
                out.string_field(file, location);
            }
            out.end_object();
            if !shard.lengths.is_empty() {
                out.start_object_field("lengths");
                for (file, length) in &shard.lengths {
                    out.int_field(file, i64::try_from(*length).unwrap_or(i64::MAX));
                }
                out.end_object();
            }
            out.end_object();
        }
        out.end_array();
        if !self.failed_shards.is_empty() {
            out.start_array_field("failed_shards");
            for shard in &self.failed_shards {
                out.int_value(i64::from(*shard));
            }
            out.end_array();
        }
        out.end_object();
    }
}

fn put_vint(buf: &mut [u8], at: &mut usize, mut n: usize) {
    while n >= 0x80 {
        // Seven bits and the continuation bit, which adding sets as `|` would.
        let low = u8::try_from(n & 0x7f).unwrap_or(0);
        put(buf, at, &[low.saturating_add(0x80)]);
        n >>= 7;
    }
    put(buf, at, &[u8::try_from(n).unwrap_or(0)]);
}

// Writes what fits and always advances, so the end is the length needed.
fn put(buf: &mut [u8], at: &mut usize, bytes: &[u8]) {
    let end = at.saturating_add(bytes.len());
    if let Some(room) = buf.get_mut(*at..end) {
        room.copy_from_slice(bytes);
    }
    *at = end;
}

impl SnapshotState<'_> {
    /// Writes the value of a held name register: the marker, the record and
    /// the finished flag, in the Java's stream encoding.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a marker that is not ASCII (its length
    /// in the stream is a character count); [`FormatError::TooLarge`] when
    /// `buf` is too short, with `actual` the length needed.
    pub fn write_held(
        marker: &str,
        record: &[u8],
        finished: bool,
        buf: &mut [u8],
    ) -> Result<usize, FormatError> {
        if !marker.is_ascii() {
            return Err(corrupt(STATE, "a marker that is not ASCII"));
        }
        let mut at = 0;
        put_vint(buf, &mut at, marker.len());
        put(buf, &mut at, marker.as_bytes());
        put_vint(buf, &mut at, record.len());
        put(buf, &mut at, record);
        put(buf, &mut at, &[u8::from(finished)]);
        if at > buf.len() {
            return Err(FormatError::TooLarge {
                what: STATE,
                limit: len64(buf.len()),
                actual: len64(at),
            });
        }
        Ok(at)
    }

    /// Writes the value of a held name register whose record is `draft`,
    /// building the record in place so one buffer serves both.
    ///
    /// # Errors
    ///
    /// As [`SnapshotState::write_held`]; `actual` is the whole value's length.
    pub fn write_held_draft(
        marker: &str,
        draft: &SnapshotDraft,
        finished: bool,
        buf: &mut [u8],
    ) -> Result<usize, FormatError> {
        if !marker.is_ascii() {
            return Err(corrupt(STATE, "a marker that is not ASCII"));
        }
        let mut head = 0;
        put_vint(buf, &mut head, marker.len());
        put(buf, &mut head, marker.as_bytes());
        // A first pass over no buffer measures the record, so its count is
        // written at its true width and the record is built where it stays.
        let record_len = match draft.write(&mut []) {
            Ok(n) => n,
            Err(FormatError::TooLarge { actual, .. }) => {
                usize::try_from(actual).unwrap_or(usize::MAX)
            }
            Err(other) => return Err(other),
        };
        let mut count = [0u8; MAX_COUNT_BYTES];
        let mut counted = 0;
        put_vint(&mut count, &mut counted, record_len);
        let body = head.saturating_add(counted);
        let end = body.saturating_add(record_len);
        let total = end.saturating_add(1);
        if total > buf.len() {
            return Err(FormatError::TooLarge {
                what: STATE,
                limit: len64(buf.len()),
                actual: len64(total),
            });
        }
        put(buf, &mut head, count.get(..counted).unwrap_or_default());
        draft.write(buf.get_mut(body..end).unwrap_or_default())?;
        if let Some(flag) = buf.get_mut(end) {
            *flag = u8::from(finished);
        }
        Ok(total)
    }

    /// The value a deleted snapshot's register is left with.
    #[must_use]
    pub fn released_value() -> &'static [u8] {
        br#"{"_released":true}"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_record::SnapshotSummary;

    fn finished() -> SnapshotDraft {
        SnapshotDraft {
            repo: "backups".into(),
            name: "daily-1".into(),
            uuid: "snapuuid00000000000001".into(),
            storage_generation: Some("gen0000000000000000001".into()),
            shallow: false,
            start_millis: 1_700_000_001_000,
            end_millis: 1_700_000_002_000,
            global_state: None,
            indices: vec![IndexCapture {
                name: "logs".into(),
                uuid: "uuidlogs0000000000000A".into(),
                number_of_shards: 1,
                mapping: Some(r#"{"properties":{"n":{"type":"long"}}}"#.into()),
                settings: None,
                shards: vec![ShardCapture {
                    shard: 0,
                    term: 2,
                    files: vec![
                        ("segments_1".into(), "t=2/_0-abc.seg#110:50".into()),
                        ("_0.cfs".into(), "t=2/_0-abc.seg#10:100".into()),
                    ],
                    lengths: vec![("segments_1".into(), 50), ("_0.cfs".into(), 100)],
                }],
                failed_shards: vec![],
                aliases: vec![],
                aliases_truncated: false,
            }],
        }
    }

    fn write(draft: &SnapshotDraft) -> Vec<u8> {
        let mut buf = vec![0u8; 4096];
        let n = draft.write(&mut buf).expect("writes");
        buf.truncate(n);
        buf
    }

    #[test]
    fn a_record_reads_back_as_it_was_drafted() {
        let mut draft = finished();
        draft.storage_generation = None;
        draft.shallow = true;
        draft.indices.push(IndexCapture {
            name: "open".into(),
            uuid: "u2".into(),
            number_of_shards: 3,
            mapping: None,
            settings: Some(br#"{"index":{"number_of_replicas":"0"}}"#.to_vec()),
            shards: vec![],
            failed_shards: vec![],
            aliases: vec![],
            aliases_truncated: false,
        });
        let bytes = write(&draft);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains(r#""settings":{"index":{"number_of_replicas":"0"}}"#),
            "{text}"
        );
        assert!(!text.contains("storage_generation"), "{text}");
        let read = SnapshotSummary::parse(&bytes).expect("parses");
        assert!(read.shallow);
        assert_eq!(read.indices.len(), 2);
        let open = read
            .indices
            .iter()
            .find(|i| i.name == "open")
            .expect("named");
        assert_eq!((open.shards, open.captured, open.capturing()), (3, 0, true));
    }

    #[test]
    fn empty_settings_and_lengths_write_nothing() {
        let mut draft = finished();
        draft.indices[0].settings = Some(b"{}".to_vec());
        draft.indices[0].shards[0].lengths.clear();
        let text = String::from_utf8_lossy(&write(&draft)).into_owned();
        assert!(
            !text.contains("settings") && !text.contains("lengths"),
            "{text}"
        );
    }

    #[test]
    fn a_short_buffer_reports_the_length_needed() {
        let need = write(&finished()).len();
        let mut buf = vec![0u8; need - 1];
        match finished().write(&mut buf) {
            Err(FormatError::TooLarge { actual, .. }) => assert_eq!(actual, len64(need)),
            other => panic!("expected TooLarge, got {other:?}"),
        }
        let mut exact = vec![0u8; need];
        assert_eq!(finished().write(&mut exact).expect("fits"), need);
    }

    #[test]
    fn a_long_record_takes_a_two_byte_count_and_a_non_ascii_marker_is_refused() {
        let record = vec![b'x'; 300];
        let mut buf = vec![0u8; 400];
        let n = SnapshotState::write_held("m", &record, false, &mut buf).expect("writes");
        assert_eq!(&buf[..4], &[1, b'm', 0xac, 0x02]);
        assert_eq!(n, 1 + 1 + 2 + 300 + 1);
        let SnapshotState::Held {
            record: back,
            finished,
            ..
        } = SnapshotState::parse(&buf[..n]).expect("parses")
        else {
            panic!("held");
        };
        assert_eq!((back.len(), finished), (300, false));
        assert!(SnapshotState::write_held("é", b"{}", true, &mut buf).is_err());
        assert_eq!(SnapshotState::released_value(), br#"{"_released":true}"#);
    }

    #[test]
    fn a_value_built_around_a_draft_equals_the_value_of_its_bytes() {
        let draft = finished();
        let record = write(&draft);
        let mut a = vec![0u8; 4096];
        let mut b = vec![0u8; 4096];
        let n = SnapshotState::write_held("v2-ab#cd#ef", &record, true, &mut a).expect("a");
        let m = SnapshotState::write_held_draft("v2-ab#cd#ef", &draft, true, &mut b).expect("b");
        assert_eq!(a[..n], b[..m]);
        // Exactly the length, one short, and too short for even the header.
        let mut exact = vec![0u8; n];
        assert_eq!(
            SnapshotState::write_held_draft("v2-ab#cd#ef", &draft, true, &mut exact).expect("fits"),
            n
        );
        for short in [n - 1, 8, 0] {
            let mut buf = vec![0u8; short];
            match SnapshotState::write_held_draft("v2-ab#cd#ef", &draft, true, &mut buf) {
                Err(FormatError::TooLarge { actual, .. }) => {
                    assert!(actual >= len64(n), "{actual}")
                }
                other => panic!("expected TooLarge, got {other:?}"),
            }
        }
        assert!(SnapshotState::write_held_draft("é", &draft, true, &mut b).is_err());
    }

    #[test]
    fn failed_shards_are_written_only_when_there_are_some_and_read_back() {
        let mut draft = finished();
        draft.indices[0].number_of_shards = 3;
        let plain = String::from_utf8(write(&draft)).unwrap();
        assert!(!plain.contains("failed_shards"), "{plain}");
        draft.indices[0].shards[0].shard = 1;
        draft.indices[0].failed_shards = vec![0, 2];
        let bytes = write(&draft);
        assert!(String::from_utf8_lossy(&bytes).contains(r#""failed_shards":[0,2]"#),);
        let s = crate::SnapshotSummary::parse(&bytes).unwrap();
        assert_eq!(s.indices[0].failed_shards, vec![0, 2]);
        assert_eq!(s.indices[0].captured, 1);
    }
}
