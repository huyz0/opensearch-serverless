//! What a snapshot record says about the snapshot, read as the shell's
//! `SnapshotRecord.fromStream` reads it, without the captured files.
//!
//! The summary has what `GET _snapshot/{repo}/{name}` answers: identity,
//! times, whether it is shallow, and for each index its uuid, shard count
//! and how many shards were captured. Mappings, settings and the file lists
//! are skipped, never copied, so a record of any size reads in memory the
//! size of its index names.

use crate::error::{FormatError, corrupt};
use crate::index_record::MAX_SHARDS;
use crate::json::{Reader, Token};
use crate::snapshot_pins::MAX_SNAPSHOT_RECORD_BYTES;

const WHAT: &str = "snapshot record";

/// One index a snapshot captured, or is capturing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotIndex {
    /// The index's name at capture.
    pub name: String,
    /// Its uuid.
    pub uuid: String,
    /// How many shards it had.
    pub shards: u32,
    /// How many shards have a captured commit; none while the capture runs.
    pub captured: u32,
}

impl SnapshotIndex {
    /// Whether the capture of this index has not finished: it has shards
    /// and none is recorded.
    #[must_use]
    pub fn capturing(&self) -> bool {
        self.captured == 0 && self.shards > 0
    }
}

/// A snapshot record, less its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummary {
    /// The repository it belongs to.
    pub repo: String,
    /// Its name.
    pub name: String,
    /// Its uuid, when the record has one.
    pub uuid: Option<String>,
    /// The unique lifetime its copied data lives under, when it has one.
    pub storage_generation: Option<String>,
    /// Whether it references the live shards' blobs instead of copying them.
    pub shallow: bool,
    /// When it started, in milliseconds since the epoch.
    pub start_millis: i64,
    /// When it ended, in milliseconds since the epoch.
    pub end_millis: i64,
    /// The indices it names, in record order.
    pub indices: Vec<SnapshotIndex>,
}

impl SnapshotSummary {
    /// Reads a snapshot record.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a missing or mistyped
    /// field the Java requires, a negative shard count or term, or a record
    /// past [`MAX_SNAPSHOT_RECORD_BYTES`].
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.len() > MAX_SNAPSHOT_RECORD_BYTES {
            return Err(corrupt(WHAT, "a record past the size limit"));
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a snapshot record is an object")?;
        let mut repo = None;
        let mut name = None;
        let mut uuid = None;
        let mut generation = None;
        let mut shallow = None;
        let mut start = None;
        let mut end = None;
        let mut indices = None;
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "repo" => repo = Some(string(&mut json, "repo")?),
                "name" => name = Some(string(&mut json, "name")?),
                "uuid" => uuid = Some(string(&mut json, "uuid")?),
                "storage_generation" => generation = Some(string(&mut json, "storage_generation")?),
                "shallow" => shallow = Some(boolean(&mut json)?),
                "start_time_in_millis" => start = Some(long(&mut json, "start_time_in_millis")?),
                "end_time_in_millis" => end = Some(long(&mut json, "end_time_in_millis")?),
                "indices" => indices = Some(read_indices(&mut json)?),
                _ => json.skip_value()?,
            }
        }
        json.finish()?;
        match (repo, name, shallow, start, end, indices) {
            (
                Some(repo),
                Some(name),
                Some(shallow),
                Some(start_millis),
                Some(end_millis),
                Some(indices),
            ) => Ok(Self {
                repo,
                name,
                uuid,
                storage_generation: generation,
                shallow,
                start_millis,
                end_millis,
                indices,
            }),
            _ => Err(corrupt(WHAT, "missing a required field")),
        }
    }

    /// Whether any index is still being captured.
    #[must_use]
    pub fn in_progress(&self) -> bool {
        self.indices.iter().any(SnapshotIndex::capturing)
    }
}

fn string(json: &mut Reader<'_>, _what: &'static str) -> Result<String, FormatError> {
    match json.expect_token()? {
        // ALLOC: one name or id, bounded by the record size.
        Token::Str(s) if !s.is_empty() => Ok(s.into_owned()),
        _ => Err(json.error("a snapshot field that is a nonempty string is not")),
    }
}

fn long(json: &mut Reader<'_>, _what: &'static str) -> Result<i64, FormatError> {
    match json.expect_token()? {
        Token::Int(n) => Ok(n),
        Token::Str(s) => s
            .parse()
            .map_err(|_| json.error("a snapshot number is not one")),
        _ => Err(json.error("a snapshot number is not one")),
    }
}

fn boolean(json: &mut Reader<'_>) -> Result<bool, FormatError> {
    match json.expect_token()? {
        Token::Bool(b) => Ok(b),
        Token::Str(s) if s == "true" => Ok(true),
        Token::Str(s) if s == "false" => Ok(false),
        _ => Err(json.error("shallow is a boolean")),
    }
}

fn read_indices(json: &mut Reader<'_>) -> Result<Vec<SnapshotIndex>, FormatError> {
    json.start_object("a snapshot's indices are an object")?;
    // ALLOC: one entry an index, bounded by the record size.
    let mut out = Vec::new();
    while let Some(name) = json.next_field()? {
        json.start_object("a snapshot's index entry is an object")?;
        let mut uuid = None;
        let mut shards = None;
        let mut listed = None;
        while let Some(key) = json.next_field()? {
            match key.as_ref() {
                "uuid" => uuid = Some(string(json, "uuid")?),
                "number_of_shards" => {
                    let n = long(json, "number_of_shards")?;
                    let n = u32::try_from(n)
                        .ok()
                        .filter(|n| *n <= MAX_SHARDS)
                        .ok_or_else(|| json.error("a shard count is out of range"))?;
                    shards = Some(n);
                }
                "settings" => match json.expect_token()? {
                    Token::StartObject => json.skip_after(&Token::StartObject)?,
                    _ => return Err(json.error("an index's settings are an object")),
                },
                "shards" => listed = Some(count_shards(json)?),
                _ => json.skip_value()?,
            }
        }
        let (Some(uuid), Some(shards)) = (uuid, shards) else {
            return Err(json.error("a snapshot's index entry misses its uuid or shard count"));
        };
        let captured = match listed {
            Some((count, highest)) if count > 0 => {
                if count != shards || highest >= shards {
                    return Err(json.error("a finished index names every shard exactly once"));
                }
                count
            }
            _ => 0,
        };
        // ALLOC: one name, as above.
        out.push(SnapshotIndex {
            name: name.into_owned(),
            uuid,
            shards,
            captured,
        });
    }
    Ok(out)
}

/// The number of entries of a `shards` array and the highest shard number,
/// each entry checked to be an object with a shard number, a non-negative
/// term and a `files` object, and no shard named twice.
fn count_shards(json: &mut Reader<'_>) -> Result<(u32, u32), FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("an index's shards are an array"));
    }
    // One bit a shard slot, MAX_SHARDS of them, on the stack.
    let mut seen = [0u64; (MAX_SHARDS / 64) as usize];
    let mut count = 0u32;
    let mut highest = 0u32;
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok((count, highest)),
            Token::StartObject => {}
            _ => return Err(json.error("a captured shard is an object")),
        }
        let mut number = None;
        let mut term_ok = false;
        let mut files = false;
        while let Some(key) = json.next_field()? {
            match key.as_ref() {
                "shard" => {
                    number = u32::try_from(long(json, "shard")?)
                        .ok()
                        .filter(|n| *n < MAX_SHARDS);
                }
                "term" => {
                    term_ok = long(json, "term")? >= 0;
                }
                "files" => {
                    files = json.next_is_object();
                    json.skip_value()?;
                }
                _ => json.skip_value()?,
            }
        }
        let Some(number) = number else {
            return Err(json.error("a captured shard has no valid shard number"));
        };
        if !term_ok || !files {
            return Err(json.error("a captured shard has no term or no files"));
        }
        let (word, bit) = ((number / 64) as usize, 1u64 << (number % 64));
        if seen[word] & bit != 0 {
            return Err(json.error("a shard is captured twice"));
        }
        seen[word] |= bit;
        highest = highest.max(number);
        count = count.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DONE: &[u8] = br#"{"repo":"r","name":"s","uuid":"U","storage_generation":"G","shallow":false,
        "start_time_in_millis":10,"end_time_in_millis":20,
        "indices":{"logs":{"uuid":"A","number_of_shards":2,"mapping":"{}","settings":{"index":{"x":[1]}},
          "shards":[{"shard":0,"term":1,"files":{"f":"t"}},{"shard":1,"term":2,"files":{},"lengths":{"f":3}}]}}}"#;

    #[test]
    fn a_finished_record_reads_to_its_summary() {
        let s = SnapshotSummary::parse(DONE).expect("reads");
        assert_eq!((s.repo.as_str(), s.name.as_str()), ("r", "s"));
        assert_eq!(s.uuid.as_deref(), Some("U"));
        assert_eq!(s.storage_generation.as_deref(), Some("G"));
        assert!(!s.shallow);
        assert_eq!((s.start_millis, s.end_millis), (10, 20));
        assert_eq!(
            s.indices,
            [SnapshotIndex {
                name: "logs".into(),
                uuid: "A".into(),
                shards: 2,
                captured: 2
            }]
        );
        assert!(!s.in_progress());
    }

    #[test]
    fn an_index_with_shards_and_no_capture_is_in_progress() {
        let s = SnapshotSummary::parse(
            br#"{"repo":"r","name":"s","shallow":"true","start_time_in_millis":"1","end_time_in_millis":2,
            "indices":{"a":{"uuid":"A","number_of_shards":1},"b":{"uuid":"B","number_of_shards":0}}}"#,
        )
        .expect("reads");
        assert!(s.shallow && s.uuid.is_none() && s.storage_generation.is_none());
        assert!(s.indices[0].capturing() && !s.indices[1].capturing());
        assert!(s.in_progress());
    }

    #[test]
    fn what_the_java_refuses_is_refused() {
        let base = |indices: &str, extra: &str| {
            format!(
                r#"{{"repo":"r","name":"s","shallow":false,"start_time_in_millis":1,"end_time_in_millis":2,{extra}"indices":{indices}}}"#
            )
        };
        for bad in [
            base(r#"{"a":{"number_of_shards":1}}"#, ""),
            base(r#"{"a":{"uuid":"A"}}"#, ""),
            base(r#"{"a":{"uuid":"A","number_of_shards":-1}}"#, ""),
            base(
                r#"{"a":{"uuid":"A","number_of_shards":1,"settings":[]}}"#,
                "",
            ),
            base(r#"{"a":{"uuid":"A","number_of_shards":1,"shards":{}}}"#, ""),
            base(
                r#"{"a":{"uuid":"A","number_of_shards":1,"shards":[{"term":-1,"files":{}}]}}"#,
                "",
            ),
            base(
                r#"{"a":{"uuid":"A","number_of_shards":1,"shards":[{"term":1}]}}"#,
                "",
            ),
            base(r#"{}"#, r#""shallow":"maybe","#),
            "{}".to_owned(),
            r#"{"repo":"r","name":"s","shallow":false,"start_time_in_millis":1,"indices":{}}"#
                .to_owned(),
            String::new(),
        ] {
            assert!(SnapshotSummary::parse(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_record_past_the_limit_is_refused() {
        let mut big = DONE.to_vec();
        big.resize(MAX_SNAPSHOT_RECORD_BYTES + 1, b' ');
        assert!(SnapshotSummary::parse(&big).is_err());
    }

    #[test]
    fn a_record_at_the_limit_is_read() {
        let mut full = DONE.to_vec();
        full.resize(MAX_SNAPSHOT_RECORD_BYTES, b' ');
        assert!(SnapshotSummary::parse(&full).is_ok());
    }

    #[test]
    fn an_empty_name_and_a_false_text_shallow_are_read_as_the_java_reads_them() {
        let with = |name: &str, shallow: &str| {
            format!(
                r#"{{"repo":"r","name":"{name}","shallow":{shallow},"start_time_in_millis":1,"end_time_in_millis":2,"indices":{{}}}}"#
            )
        };
        assert!(SnapshotSummary::parse(with("", "false").as_bytes()).is_err());
        let off = SnapshotSummary::parse(with("s", r#""false""#).as_bytes()).expect("reads");
        assert!(!off.shallow);
        let on = SnapshotSummary::parse(with("s", r#""true""#).as_bytes()).expect("reads");
        assert!(on.shallow);
    }

    fn record_of(count: u32, shard_numbers: &[u32]) -> String {
        let shards = shard_numbers
            .iter()
            .map(|n| format!(r#"{{"shard":{n},"term":1,"files":{{}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"repo":"r","name":"s","shallow":false,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{{"a":{{"uuid":"A","number_of_shards":{count},"shards":[{shards}]}}}}}}"#
        )
    }

    #[test]
    fn a_finished_index_names_every_shard_exactly_once() {
        for (count, shards) in [
            (3, vec![2, 0, 1]),
            (2, vec![0, 2]),
            (3, vec![0, 1]),
            (2, vec![0, 0]),
            (2, vec![0, 1, 1]),
            (1, vec![1]),
        ] {
            let record = record_of(count, &shards);
            if shards.len() == count as usize && shards.iter().all(|s| *s < count) {
                let mut seen = shards.clone();
                seen.sort_unstable();
                seen.dedup();
                if seen.len() == count as usize {
                    assert!(
                        SnapshotSummary::parse(record.as_bytes()).is_ok(),
                        "{record}"
                    );
                    continue;
                }
            }
            assert!(
                SnapshotSummary::parse(record.as_bytes()).is_err(),
                "{record}"
            );
        }
    }

    #[test]
    fn a_shard_without_its_number_is_refused() {
        let record = r#"{"repo":"r","name":"s","shallow":false,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{"a":{"uuid":"A","number_of_shards":1,"shards":[{"term":1,"files":{}}]}}}"#;
        assert!(SnapshotSummary::parse(record.as_bytes()).is_err());
    }

    #[test]
    fn the_shard_count_is_bounded_before_anything_is_allocated() {
        let all: Vec<u32> = (0..4096).collect();
        assert!(SnapshotSummary::parse(record_of(4096, &all).as_bytes()).is_ok());
        let over: Vec<u32> = (0..4097).collect();
        assert!(SnapshotSummary::parse(record_of(4097, &over).as_bytes()).is_err());
    }

    #[test]
    fn an_index_with_no_shards_listed_is_still_the_marker() {
        let record = r#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{"a":{"uuid":"A","number_of_shards":5}}}"#;
        let s = SnapshotSummary::parse(record.as_bytes()).expect("reads");
        assert!(s.indices[0].capturing());
    }
}
