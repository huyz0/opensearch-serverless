//! The aliases one index of a snapshot record carries, written by a take
//! and read by a restore (ADR 0065): `"aliases": {"<name>": {"is_write_index":
//! true}}` and `"aliases_truncated": true` on the index's entry.

use crate::error::FormatError;
use crate::json::out::Out;
use crate::json::{Reader, Token};

/// The most aliases one index entry records.
pub const MAX_ALIASES: usize = 64;
/// The longest alias name a record carries, in bytes.
pub const MAX_ALIAS_NAME_BYTES: usize = 255;

/// One alias an index was in when the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasCapture {
    /// The alias's name.
    pub name: String,
    /// Whether the index was the alias's explicit write index.
    pub write: bool,
}

/// Writes `aliases` and the truncation flag into an open index entry; nothing
/// when there is neither.
pub(crate) fn write(out: &mut Out<'_>, aliases: &[AliasCapture], truncated: bool) {
    if !aliases.is_empty() {
        out.start_object_field("aliases");
        for alias in aliases {
            out.start_object_field(&alias.name);
            if alias.write {
                out.bool_field("is_write_index", true);
            }
            out.end_object();
        }
        out.end_object();
    }
    if truncated {
        out.bool_field("aliases_truncated", true);
    }
}

/// Reads the value of an index entry's `aliases` field.
pub(crate) fn read(json: &mut Reader<'_>) -> Result<Vec<AliasCapture>, FormatError> {
    json.start_object("an index's aliases are an object")?;
    // ALLOC: one entry an alias, at most MAX_ALIASES.
    let mut found: Vec<AliasCapture> = Vec::new();
    while let Some(name) = json.next_field()? {
        if name.is_empty() || name.len() > MAX_ALIAS_NAME_BYTES {
            return Err(json.error("an alias name is 1 to 255 bytes"));
        }
        if found.len() >= MAX_ALIASES {
            return Err(json.error("an index records at most 64 aliases"));
        }
        if found.iter().any(|a| a.name == name) {
            return Err(json.error("an alias is named twice"));
        }
        json.start_object("an alias entry is an object")?;
        let mut write = false;
        while let Some(key) = json.next_field()? {
            if key == "is_write_index" {
                match json.expect_token()? {
                    Token::Bool(b) => write = b,
                    _ => return Err(json.error("is_write_index is a boolean")),
                }
            } else {
                json.skip_value()?;
            }
        }
        // ALLOC: one name, at most MAX_ALIAS_NAME_BYTES.
        found.push(AliasCapture {
            name: name.into_owned(),
            write,
        });
    }
    Ok(found)
}

/// Reads the value of an index entry's `aliases_truncated` field.
pub(crate) fn read_truncated(json: &mut Reader<'_>) -> Result<bool, FormatError> {
    match json.expect_token()? {
        Token::Bool(b) => Ok(b),
        _ => Err(json.error("aliases_truncated is a boolean")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_restore::RestoreIndex;
    use crate::snapshot_write::{IndexCapture, SnapshotDraft};

    fn draft(aliases: Vec<AliasCapture>, truncated: bool) -> SnapshotDraft {
        SnapshotDraft {
            repo: "r".into(),
            name: "s".into(),
            uuid: "u".into(),
            storage_generation: None,
            shallow: true,
            start_millis: 1,
            end_millis: 2,
            indices: vec![IndexCapture {
                name: "logs".into(),
                uuid: "U".into(),
                number_of_shards: 1,
                mapping: None,
                settings: None,
                shards: vec![],
                failed_shards: vec![],
                aliases,
                aliases_truncated: truncated,
            }],
        }
    }

    fn bytes(d: &SnapshotDraft) -> Vec<u8> {
        let mut buf = vec![0u8; 8192];
        let n = d.write(&mut buf).unwrap();
        buf.truncate(n);
        buf
    }

    fn alias(name: &str, write: bool) -> AliasCapture {
        AliasCapture {
            name: name.into(),
            write,
        }
    }

    fn record(entry: &str) -> Vec<u8> {
        format!(r#"{{"indices":{{"logs":{{"uuid":"U","number_of_shards":1,{entry}}}}}}}"#)
            .into_bytes()
    }

    fn read_entry(entry: &str) -> Result<RestoreIndex, FormatError> {
        RestoreIndex::find(&record(entry), "logs").map(|i| i.unwrap())
    }

    #[test]
    fn aliases_written_are_read_back_and_a_write_flag_survives() {
        let d = draft(vec![alias("a", true), alias("b", false)], true);
        let b = bytes(&d);
        let text = String::from_utf8_lossy(&b);
        assert!(
            text.contains(r#""aliases":{"a":{"is_write_index":true},"b":{}}"#),
            "{text}"
        );
        assert!(text.contains(r#""aliases_truncated":true"#), "{text}");
        let got = RestoreIndex::find(&b, "logs").unwrap().unwrap();
        assert_eq!(got.aliases, [alias("a", true), alias("b", false)]);
        assert!(got.aliases_truncated);
    }

    #[test]
    fn an_index_with_none_writes_neither_field() {
        let b = bytes(&draft(vec![], false));
        let text = String::from_utf8_lossy(&b);
        assert!(!text.contains("aliases"), "{text}");
        let got = RestoreIndex::find(&b, "logs").unwrap().unwrap();
        assert!(got.aliases.is_empty() && !got.aliases_truncated);
    }

    #[test]
    fn a_write_flag_absent_false_or_true_reads_as_the_flag() {
        let got = read_entry(
            r#""aliases":{"a":{},"b":{"is_write_index":false},"c":{"is_write_index":true,"x":1}}"#,
        )
        .unwrap();
        assert_eq!(
            got.aliases,
            [alias("a", false), alias("b", false), alias("c", true)]
        );
    }

    #[test]
    fn a_malformed_field_is_a_corrupt_record() {
        for bad in [
            r#""aliases":[]"#,
            r#""aliases":{"a":true}"#,
            r#""aliases":{"a":{"is_write_index":"yes"}}"#,
            r#""aliases":{"":{}}"#,
            r#""aliases":{"a":{},"a":{}}"#,
            r#""aliases_truncated":"yes""#,
            r#""aliases_truncated":1"#,
        ] {
            assert!(read_entry(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_bounds_are_checked_at_the_edge() {
        let name = |n: usize| "a".repeat(n);
        let ok = format!(r#""aliases":{{"{}":{{}}}}"#, name(MAX_ALIAS_NAME_BYTES));
        assert_eq!(read_entry(&ok).unwrap().aliases.len(), 1);
        let long = format!(r#""aliases":{{"{}":{{}}}}"#, name(MAX_ALIAS_NAME_BYTES + 1));
        assert!(read_entry(&long).is_err());
        let many = |n: usize| {
            let body: Vec<String> = (0..n).map(|i| format!(r#""a{i}":{{}}"#)).collect();
            format!(r#""aliases":{{{}}}"#, body.join(","))
        };
        assert_eq!(read_entry(&many(MAX_ALIASES)).unwrap().aliases.len(), 64);
        assert!(read_entry(&many(MAX_ALIASES + 1)).is_err());
    }

    #[test]
    fn a_record_with_no_alias_fields_reads_as_none() {
        let got = read_entry(r#""shards":[]"#).unwrap();
        assert!(got.aliases.is_empty() && !got.aliases_truncated);
    }

    #[test]
    fn the_summary_parse_ignores_the_fields_even_when_malformed() {
        let mut d = draft(vec![alias("a", true)], false);
        let mut ok = bytes(&d);
        assert!(crate::SnapshotSummary::parse(&ok).is_ok());
        d.indices[0].aliases.clear();
        let plain = String::from_utf8(bytes(&d)).unwrap();
        ok = plain
            .replace(
                r#""shards":[]"#,
                r#""aliases":[1,2],"aliases_truncated":"x","shards":[]"#,
            )
            .into_bytes();
        assert!(String::from_utf8_lossy(&ok).contains("aliases_truncated"));
        assert!(crate::SnapshotSummary::parse(&ok).is_ok());
    }
}
