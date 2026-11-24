//! A snapshot repository's record, read as the shell's
//! `RepositoryDescriptor.fromStream` reads it.
//!
//! A JSON object with `name` and `created_at` (both required), `type`
//! (`fs`, the native type, when absent) and `settings` (an object of scalar
//! values, each read as the text the Java's `String.valueOf` gives it).

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};

const WHAT: &str = "repository record";

/// The longest record read: a repository record is a few hundred bytes.
pub const MAX_REPOSITORY_RECORD_BYTES: usize = 1 << 20;

/// The type a record with none has: the deployment's own object store.
pub const NATIVE_TYPE: &str = "fs";

/// A registered repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRecord {
    /// The repository's name.
    pub name: String,
    /// Its type; [`NATIVE_TYPE`] when the record names none.
    pub repo_type: String,
    /// When it was registered, in milliseconds since the epoch.
    pub created_at_millis: i64,
    /// Its settings, in record order, each value as text.
    pub settings: Vec<(String, String)>,
}

impl RepositoryRecord {
    /// Reads a repository record.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a missing or mistyped
    /// `name` or `created_at`, a setting that is null, an object or an
    /// array, or a record past [`MAX_REPOSITORY_RECORD_BYTES`].
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.len() > MAX_REPOSITORY_RECORD_BYTES {
            return Err(corrupt(WHAT, "a record past the size limit"));
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a repository record is an object")?;
        let mut name = None;
        let mut repo_type = None;
        let mut created = None;
        // ALLOC: one entry a setting, bounded by the record size.
        let mut settings = Vec::new();
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "name" => name = Some(text(&mut json, "a repository's name is a string")?),
                "type" => repo_type = Some(text(&mut json, "a repository's type is a string")?),
                "created_at" => match json.expect_token()? {
                    Token::Int(n) => created = Some(n),
                    Token::Str(s) => {
                        created = Some(
                            s.parse()
                                .map_err(|_| json.error("created_at is not a number"))?,
                        );
                    }
                    _ => return Err(json.error("created_at is a number")),
                },
                "settings" => read_settings(&mut json, &mut settings)?,
                _ => json.skip_value()?,
            }
        }
        json.finish()?;
        let (Some(name), Some(created_at_millis)) = (name, created) else {
            return Err(corrupt(WHAT, "missing a required field"));
        };
        Ok(Self {
            name,
            // ALLOC: one type name, once a record.
            repo_type: repo_type.unwrap_or_else(|| NATIVE_TYPE.to_owned()),
            created_at_millis,
            settings,
        })
    }

    /// Whether snapshots here are shallow unless a request says otherwise.
    #[must_use]
    pub fn shallow_by_default(&self) -> bool {
        self.settings
            .iter()
            .any(|(k, v)| k == "remote_store_index_shallow_copy" && v == "true")
    }
}

fn text(json: &mut Reader<'_>, reason: &'static str) -> Result<String, FormatError> {
    match json.expect_token()? {
        // ALLOC: one name or type, bounded by the record size.
        Token::Str(s) => Ok(s.into_owned()),
        _ => Err(json.error(reason)),
    }
}

fn read_settings(
    json: &mut Reader<'_>,
    settings: &mut Vec<(String, String)>,
) -> Result<(), FormatError> {
    match json.expect_token()? {
        Token::StartObject => {}
        // The Java ignores a `settings` that is not an object.
        other => return json.skip_after(&other),
    }
    while let Some(key) = json.next_field()? {
        // ALLOC: one value a setting, bounded by the record size.
        let value = match json.expect_token()? {
            Token::Str(s) => s.into_owned(),
            Token::Int(_) | Token::OtherNumber => json.number_text().to_owned(),
            // ALLOC: one bool as text, a setting's value.
            Token::Bool(b) => b.to_string(),
            _ => return Err(json.error("a repository setting is a string, number or boolean")),
        };
        // ALLOC: one entry a setting, bounded by the record size.
        settings.push((key.into_owned(), value));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_the_java_wrote_is_read() {
        let r = RepositoryRecord::parse(
            br#"{"name":"backups","type":"fs","created_at":1700000000000,"settings":{"location":"x","remote_store_index_shallow_copy":true,"max":5}}"#,
        )
        .expect("reads");
        assert_eq!(r.name, "backups");
        assert_eq!(r.repo_type, "fs");
        assert_eq!(r.created_at_millis, 1_700_000_000_000);
        assert_eq!(
            r.settings,
            [
                ("location".to_owned(), "x".to_owned()),
                (
                    "remote_store_index_shallow_copy".to_owned(),
                    "true".to_owned()
                ),
                ("max".to_owned(), "5".to_owned()),
            ]
        );
        assert!(r.shallow_by_default());
    }

    #[test]
    fn a_missing_type_is_native_and_missing_settings_are_none() {
        let r = RepositoryRecord::parse(br#"{"name":"r","created_at":"7"}"#).expect("reads");
        assert_eq!((r.repo_type.as_str(), r.created_at_millis), ("fs", 7));
        assert!(r.settings.is_empty() && !r.shallow_by_default());
    }

    #[test]
    fn a_record_missing_a_required_field_or_malformed_is_refused() {
        for bad in [
            &br#"{"created_at":1}"#[..],
            br#"{"name":"r"}"#,
            br#"{"name":3,"created_at":1}"#,
            br#"{"name":"r","created_at":"x"}"#,
            br#"{"name":"r","created_at":1,"settings":{"a":null}}"#,
            br#"{"name":"r","created_at":1,"settings":{"a":{}}}"#,
            br#"[]"#,
            b"",
        ] {
            assert!(RepositoryRecord::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_settings_that_is_not_an_object_is_ignored_as_the_java_does() {
        let r = RepositoryRecord::parse(br#"{"name":"r","created_at":1,"settings":[1,2]}"#)
            .expect("reads");
        assert!(r.settings.is_empty());
    }

    #[test]
    fn a_record_past_the_limit_is_refused() {
        let mut big = br#"{"name":"r","created_at":1}"#.to_vec();
        big.resize(MAX_REPOSITORY_RECORD_BYTES + 1, b' ');
        assert!(RepositoryRecord::parse(&big).is_err());
    }

    #[test]
    fn a_record_at_the_limit_is_read() {
        let mut full = br#"{"name":"r","created_at":1}"#.to_vec();
        full.resize(MAX_REPOSITORY_RECORD_BYTES, b' ');
        assert!(RepositoryRecord::parse(&full).is_ok());
    }

    #[test]
    fn a_named_type_is_kept() {
        let r =
            RepositoryRecord::parse(br#"{"name":"r","type":"s3","created_at":1}"#).expect("reads");
        assert_eq!(r.repo_type, "s3");
    }

    #[test]
    fn only_the_shallow_setting_set_true_makes_snapshots_shallow() {
        let read = |settings: &str| {
            RepositoryRecord::parse(
                format!(r#"{{"name":"r","created_at":1,"settings":{settings}}}"#).as_bytes(),
            )
            .expect("reads")
            .shallow_by_default()
        };
        assert!(read(r#"{"remote_store_index_shallow_copy":"true"}"#));
        assert!(!read(r#"{"remote_store_index_shallow_copy":false}"#));
        assert!(!read(r#"{"other":true}"#));
    }
}
