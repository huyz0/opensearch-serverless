//! One index of a snapshot record, read whole for a restore: its uuid,
//! shard count, mapping, settings and each captured shard's files, as the
//! shell's `SnapshotRecord.fromStream` reads them. The rest of the record
//! is skipped, so memory is the one index's lists.

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};
use crate::location::FileLocation;
use crate::snapshot_pins::MAX_SNAPSHOT_RECORD_BYTES;

const WHAT: &str = "snapshot record";

/// One captured shard: its commit's files, each with the term directory (or
/// pack range text) it was published under, and their lengths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreShard {
    /// The shard number.
    pub shard: u32,
    /// The term the commit was published under.
    pub term: u64,
    /// File name and location, in record order.
    pub files: Vec<(String, String)>,
    /// File name and length, in record order; empty for an old record.
    pub lengths: Vec<(String, u64)>,
}

/// One index of a snapshot record, with what a restore creates it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreIndex {
    /// The index's name at capture.
    pub name: String,
    /// Its uuid at capture.
    pub uuid: String,
    /// How many shards it had.
    pub number_of_shards: u32,
    /// Its mapping, the JSON text, when it had one.
    pub mapping: Option<String>,
    /// Its settings, a JSON object, when it had any.
    pub settings: Option<Vec<u8>>,
    /// The shards captured, in record order.
    pub shards: Vec<RestoreShard>,
}

impl RestoreIndex {
    /// The index called `index` in `record`; `None` when the record does
    /// not name it.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a record past
    /// [`MAX_SNAPSHOT_RECORD_BYTES`], or an index entry the Java refuses: no
    /// uuid or shard count, a negative count, term, shard or length, or a
    /// file whose location is not a string.
    pub fn find(record: &[u8], index: &str) -> Result<Option<Self>, FormatError> {
        if record.len() > MAX_SNAPSHOT_RECORD_BYTES {
            return Err(corrupt(WHAT, "a record past the size limit"));
        }
        let mut json = Reader::new(record, WHAT);
        json.start_object("a snapshot record is an object")?;
        let mut found = None;
        while let Some(field) = json.next_field()? {
            if field != "indices" {
                json.skip_value()?;
                continue;
            }
            json.start_object("a snapshot's indices are an object")?;
            while let Some(name) = json.next_field()? {
                if name == index {
                    found = Some(read_index(&mut json, index)?);
                } else {
                    json.skip_value()?;
                }
            }
        }
        json.finish()?;
        Ok(found)
    }
}

impl RestoreIndex {
    /// Why a restore refuses this entry before writing anything, when it
    /// does: the entry carries a shard mode's unit layout, settings that
    /// are another index's own identity, or a file whose length the record
    /// does not state (the Java lists a term to find it, which costs a LIST).
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] when the settings or a file's location are
    /// malformed, or a shard is numbered past the shard count.
    pub fn refusal(&self) -> Result<Option<&'static str>, FormatError> {
        if let Some(settings) = &self.settings {
            let mut json = Reader::new(settings, WHAT);
            json.start_object("an index's settings are an object")?;
            if let Some(why) = settings_refusal(&mut json, "")? {
                return Ok(Some(why));
            }
            json.finish()?;
        }
        for shard in &self.shards {
            if shard.shard >= self.number_of_shards {
                return Err(corrupt(WHAT, "a shard is numbered past the shard count"));
            }
            for (name, location) in &shard.files {
                let stated = shard.lengths.iter().any(|(n, _)| n == name)
                    || FileLocation::parse(location)?.range().is_some();
                if !stated {
                    return Ok(Some("a captured file has no recorded length"));
                }
            }
        }
        Ok(None)
    }
}

/// The unit layout's key in a captured index's settings.
const LAYOUT_KEY: &str = "serverless_unit_layout";

/// The settings a restored index never inherits: they name the captured
/// index itself.
const IDENTITY_SETTINGS: [&str; 8] = [
    "index.uuid",
    "index.history.uuid",
    "index.version.created",
    "index.version.upgraded",
    "index.creation_date",
    "index.provided_name",
    "index.number_of_shards",
    "index.number_of_replicas",
];

/// Walks one settings object (its `{` read), dotting nested names, and
/// answers the first refusal.
fn settings_refusal(
    json: &mut Reader<'_>,
    prefix: &str,
) -> Result<Option<&'static str>, FormatError> {
    while let Some(key) = json.next_field()? {
        // ALLOC: one dotted name, bounded by the settings' size.
        let name = if prefix.is_empty() {
            key.into_owned()
        } else {
            [prefix, ".", key.as_ref()].concat()
        };
        if name == LAYOUT_KEY {
            return Ok(Some("the index carries a shard mode's unit layout"));
        }
        if json.next_is_object() {
            json.start_object("a setting group is an object")?;
            if let Some(why) = settings_refusal(json, &name)? {
                return Ok(Some(why));
            }
            continue;
        }
        let full = if name.starts_with("index.") {
            name
        } else {
            // ALLOC: one prefixed name, bounded by the settings' size.
            ["index.", &name].concat()
        };
        if IDENTITY_SETTINGS.contains(&full.as_str()) {
            return Ok(Some("the index's settings carry another index's identity"));
        }
        json.skip_value()?;
    }
    Ok(None)
}

fn read_index(json: &mut Reader<'_>, name: &str) -> Result<RestoreIndex, FormatError> {
    json.start_object("a snapshot's index entry is an object")?;
    let mut uuid = None;
    let mut count = None;
    let mut mapping = None;
    let mut settings = None;
    // ALLOC: one entry a captured shard, bounded by the record size.
    let mut shards = Vec::new();
    while let Some(key) = json.next_field()? {
        match key.as_ref() {
            "uuid" => match json.expect_token()? {
                // ALLOC: one id, bounded by the record size.
                Token::Str(s) if !s.is_empty() => uuid = Some(s.into_owned()),
                _ => return Err(json.error("an index's uuid is a nonempty string")),
            },
            "number_of_shards" => {
                count = Some(
                    u32::try_from(number(json)?)
                        .map_err(|_| json.error("a shard count is out of range"))?,
                );
            }
            "mapping" => match json.expect_token()? {
                // ALLOC: one mapping, bounded by the record size.
                Token::Str(s) => mapping = Some(s.into_owned()),
                Token::Null => {}
                _ => return Err(json.error("an index's mapping is a string")),
            },
            "settings" => {
                let raw = json.raw_object("an index's settings are an object")?;
                // ALLOC: one settings object, bounded by the record size.
                settings = Some(raw.to_vec());
            }
            "shards" => read_shards(json, &mut shards)?,
            _ => json.skip_value()?,
        }
    }
    let (Some(uuid), Some(number_of_shards)) = (uuid, count) else {
        return Err(json.error("a snapshot's index entry misses its uuid or shard count"));
    };
    Ok(RestoreIndex {
        // ALLOC: one name.
        name: name.to_owned(),
        uuid,
        number_of_shards,
        mapping,
        settings,
        shards,
    })
}

fn number(json: &mut Reader<'_>) -> Result<i64, FormatError> {
    match json.expect_token()? {
        Token::Int(n) => Ok(n),
        Token::Str(s) => s
            .parse()
            .map_err(|_| json.error("a snapshot number is not one")),
        _ => Err(json.error("a snapshot number is not one")),
    }
}

fn read_shards(json: &mut Reader<'_>, out: &mut Vec<RestoreShard>) -> Result<(), FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("an index's shards are an array"));
    }
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(()),
            Token::StartObject => {}
            _ => return Err(json.error("a captured shard is an object")),
        }
        let mut shard = None;
        let mut term = None;
        // ALLOC: one entry a file, bounded by the record size.
        let mut files = None;
        // ALLOC: as above.
        let mut lengths = Vec::new();
        while let Some(key) = json.next_field()? {
            match key.as_ref() {
                "shard" => {
                    shard = Some(
                        u32::try_from(number(json)?)
                            .map_err(|_| json.error("a shard number is out of range"))?,
                    );
                }
                "term" => {
                    term = Some(
                        u64::try_from(number(json)?)
                            .map_err(|_| json.error("a term is negative"))?,
                    );
                }
                "files" => files = Some(read_files(json)?),
                "lengths" => {
                    json.start_object("a shard's lengths are an object")?;
                    while let Some(file) = json.next_field()? {
                        let length = u64::try_from(number(json)?)
                            .map_err(|_| json.error("a file length is negative"))?;
                        // ALLOC: one name, bounded by the record size.
                        lengths.push((file.into_owned(), length));
                    }
                }
                _ => json.skip_value()?,
            }
        }
        let (Some(shard), Some(term), Some(files)) = (shard, term, files) else {
            return Err(json.error("a captured shard has no number, term or files"));
        };
        out.push(RestoreShard {
            shard,
            term,
            files,
            lengths,
        });
    }
}

fn read_files(json: &mut Reader<'_>) -> Result<Vec<(String, String)>, FormatError> {
    json.start_object("a shard's files are an object")?;
    // ALLOC: one entry a file, bounded by the record size.
    let mut files = Vec::new();
    while let Some(file) = json.next_field()? {
        match json.expect_token()? {
            // ALLOC: one name and one location, bounded by the record size.
            Token::Str(location) => files.push((file.into_owned(), location.into_owned())),
            _ => return Err(json.error("a file's location is a string")),
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: &[u8] = br#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
        "indices":{
          "skip":{"uuid":"X","number_of_shards":1,"shards":[{"shard":0,"term":1,"files":{"a":"t=1"}}]},
          "logs":{"uuid":"U","number_of_shards":2,"mapping":"{\"p\":1}","settings":{"index":{"x":[1]}},
            "shards":[{"shard":0,"term":3,"files":{"f":"t=3","g":"t=1"},"lengths":{"f":7,"g":9}},
                      {"shard":1,"term":1,"files":{}}]}}}"#;

    #[test]
    fn one_index_is_read_whole_and_the_rest_skipped() {
        let got = RestoreIndex::find(RECORD, "logs").unwrap().unwrap();
        assert_eq!(got.name, "logs");
        assert_eq!(got.uuid, "U");
        assert_eq!(got.number_of_shards, 2);
        assert_eq!(got.mapping.as_deref(), Some(r#"{"p":1}"#));
        assert_eq!(
            got.settings.as_deref(),
            Some(&br#"{"index":{"x":[1]}}"#[..])
        );
        assert_eq!(got.shards.len(), 2);
        assert_eq!(got.shards[0].shard, 0);
        assert_eq!(got.shards[0].term, 3);
        assert_eq!(
            got.shards[0].files,
            [
                ("f".to_owned(), "t=3".to_owned()),
                ("g".to_owned(), "t=1".to_owned())
            ]
        );
        assert_eq!(
            got.shards[0].lengths,
            [("f".to_owned(), 7), ("g".to_owned(), 9)]
        );
        assert!(got.shards[1].files.is_empty() && got.shards[1].lengths.is_empty());
        assert_eq!(RestoreIndex::find(RECORD, "absent").unwrap(), None);
    }

    #[test]
    fn the_record_the_java_wrote_is_read() {
        let java = include_bytes!("../../../fixtures/snapshots/snapshot-finished.json");
        let got = RestoreIndex::find(java, "logs").unwrap().unwrap();
        assert_eq!(got.uuid, "uuidlogs0000000000000A");
        assert_eq!(got.shards[0].term, 2);
        assert_eq!(got.shards[0].files.len(), 2);
        assert_eq!(got.shards[0].lengths[1], ("_0.cfs".to_owned(), 100));
        assert!(got.mapping.is_some() && got.settings.is_none());
    }

    #[test]
    fn what_the_java_refuses_is_refused() {
        let with = |entry: &str| {
            format!(
                r#"{{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{{"a":{entry}}}}}"#
            )
        };
        for bad in [
            with(r#"{"number_of_shards":1}"#),
            with(r#"{"uuid":"U"}"#),
            with(r#"{"uuid":"","number_of_shards":1}"#),
            with(r#"{"uuid":"U","number_of_shards":-1}"#),
            with(r#"{"uuid":"U","number_of_shards":1,"settings":[]}"#),
            with(r#"{"uuid":"U","number_of_shards":1,"mapping":5}"#),
            with(r#"{"uuid":"U","number_of_shards":1,"shards":{}}"#),
            with(
                r#"{"uuid":"U","number_of_shards":1,"shards":[{"shard":0,"term":-1,"files":{}}]}"#,
            ),
            with(r#"{"uuid":"U","number_of_shards":1,"shards":[{"shard":0,"term":1}]}"#),
            with(
                r#"{"uuid":"U","number_of_shards":1,"shards":[{"shard":0,"term":1,"files":{"f":1}}]}"#,
            ),
            with(
                r#"{"uuid":"U","number_of_shards":1,"shards":[{"shard":0,"term":1,"files":{},"lengths":{"f":-1}}]}"#,
            ),
            with(r#"{"uuid":"U","number_of_shards":1,"shards":[{"term":1,"files":{}}]}"#),
            "{".to_owned(),
            String::new(),
        ] {
            assert!(RestoreIndex::find(bad.as_bytes(), "a").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_record_cut_short_is_refused_at_every_length() {
        for n in 0..RECORD.len() {
            assert!(
                RestoreIndex::find(RECORD.get(..n).unwrap(), "logs").is_err(),
                "{n}"
            );
        }
    }

    #[test]
    fn a_null_mapping_and_a_quoted_number_are_read_as_the_java_reads_them() {
        let record = br#"{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
            "indices":{"a":{"uuid":"U","number_of_shards":"3","mapping":null,
            "shards":[{"shard":"0","term":"2","files":{"f":"t=2"},"lengths":{"f":"5"}}]}}}"#;
        let got = RestoreIndex::find(record, "a").unwrap().unwrap();
        assert_eq!(got.number_of_shards, 3);
        assert_eq!(got.mapping, None);
        assert_eq!(got.shards[0].term, 2);
        assert_eq!(got.shards[0].lengths, [("f".to_owned(), 5)]);
    }

    fn entry(settings: &str, shards: &str) -> RestoreIndex {
        let record = format!(
            r#"{{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
            "indices":{{"a":{{"uuid":"U","number_of_shards":2,"settings":{settings},"shards":{shards}}}}}}}"#
        );
        RestoreIndex::find(record.as_bytes(), "a").unwrap().unwrap()
    }

    #[test]
    fn what_a_restore_refuses_before_writing_is_named() {
        let ok = r#"[{"shard":0,"term":1,"files":{"f":"t=1"},"lengths":{"f":3}}]"#;
        assert_eq!(entry(r#"{"index":{"x":1}}"#, ok).refusal().unwrap(), None);
        for settings in [
            r#"{"serverless_unit_layout":"{}"}"#,
            r#"{"index.uuid":"x"}"#,
            r#"{"uuid":"x"}"#,
            r#"{"index":{"uuid":"x"}}"#,
            r#"{"index":{"version":{"created":"1"}}}"#,
            r#"{"index.number_of_replicas":1}"#,
            r#"{"creation_date":"1"}"#,
        ] {
            assert!(
                entry(settings, ok).refusal().unwrap().is_some(),
                "{settings}"
            );
        }
        // A setting that only starts like an identity one is carried.
        assert_eq!(
            entry(r#"{"index.uuid_hint":1}"#, ok).refusal().unwrap(),
            None
        );
        let no_length = r#"[{"shard":0,"term":1,"files":{"f":"t=1"}}]"#;
        assert!(entry("{}", no_length).refusal().unwrap().is_some());
        let packed = r#"[{"shard":0,"term":1,"files":{"f":"t=1/b#4:9"}}]"#;
        assert_eq!(entry("{}", packed).refusal().unwrap(), None);
        let past = r#"[{"shard":2,"term":1,"files":{}}]"#;
        assert!(entry("{}", past).refusal().is_err());
    }

    #[test]
    fn a_record_of_exactly_the_limit_is_read() {
        let mut full = RECORD.to_vec();
        full.resize(MAX_SNAPSHOT_RECORD_BYTES, b' ');
        assert!(RestoreIndex::find(&full, "logs").unwrap().is_some());
    }

    #[test]
    fn a_record_past_the_limit_is_refused() {
        let mut big = RECORD.to_vec();
        big.resize(MAX_SNAPSHOT_RECORD_BYTES + 1, b' ');
        assert!(RestoreIndex::find(&big, "logs").is_err());
    }
}
