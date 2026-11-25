//! An index's metadata blob (`meta-<id>.dat`): what a restore creates the
//! index from.
//!
//! Ported from `IndexMetadata.Builder.fromXContent`, reading only what a
//! restore needs: the settings, the one mapping and the alias names. The
//! input is the JSON of the decoded blob; everything else is skipped.

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};

const WHAT: &str = "stock index metadata";

/// The most settings one index may carry.
pub const MAX_SETTINGS: usize = 4_096;
/// The most aliases one index may carry.
pub const MAX_ALIASES: usize = 10_000;
/// The only mapping type a typeless index has.
const DOC_TYPE: &str = "_doc";

/// An index's metadata, as a restore reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockIndexMetadata {
    /// The index's name when it was snapshotted.
    pub name: String,
    /// Whether the index was open (`open`) or closed (`close`).
    pub state: String,
    /// The settings, flat keys (`index.refresh_interval`) to text, in file
    /// order.
    pub settings: Vec<(String, String)>,
    /// The mapping: the JSON object of the one `_doc` type.
    pub mapping: Option<String>,
    /// The names of the aliases that pointed at the index.
    pub aliases: Vec<String>,
}

impl StockIndexMetadata {
    /// Reads the JSON of an index metadata blob.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a shape the Java does not write,
    /// [`FormatError::TooLarge`] past the setting or alias limit and
    /// [`FormatError::NotFollowable`] for what a restore does not carry: a
    /// list-valued setting or a mapping of another type than `_doc`.
    pub fn parse(json: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(json, WHAT);
        r.start_object("index metadata is an object")?;
        let Some(name) = r.next_field()? else {
            return Err(r.error("index metadata names no index"));
        };
        let name = name.into_owned();
        r.start_object("an index entry is an object")?;
        let mut out = Self {
            name,
            state: String::new(), // ALLOC: empty until filled.
            settings: Vec::new(), // ALLOC: empty until filled.
            mapping: None,
            aliases: Vec::new(), // ALLOC: empty until filled.
        };
        while let Some(field) = r.next_field()? {
            match field.as_ref() {
                "state" => match r.expect_token()? {
                    Token::Str(s) => out.state = s.into_owned(),
                    _ => return Err(r.error("a state is a string")),
                },
                "settings" => out.settings = settings(&mut r)?,
                "mappings" => out.mapping = mapping(&mut r)?,
                "aliases" => out.aliases = aliases(&mut r)?,
                _ => r.skip_value()?,
            }
        }
        if r.next_field()?.is_some() {
            return Err(r.error("index metadata names more than one index"));
        }
        r.finish()?;
        if out.state.is_empty() {
            return Err(corrupt(WHAT, "an index without a state"));
        }
        Ok(out)
    }

    /// The setting `key`, when set.
    pub fn setting(&self, key: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

fn settings(r: &mut Reader<'_>) -> Result<Vec<(String, String)>, FormatError> {
    r.start_object("settings is an object")?;
    // ALLOC: one pair a setting, at most MAX_SETTINGS.
    let mut out = Vec::new();
    while let Some(key) = r.next_field()? {
        if out.len() >= MAX_SETTINGS {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_SETTINGS as u64,
                actual: (MAX_SETTINGS as u64).saturating_add(1),
            });
        }
        let key = key.into_owned();
        match r.expect_token()? {
            Token::Str(v) => out.push((key, v.into_owned())),
            Token::StartArray => {
                return Err(FormatError::NotFollowable {
                    what: WHAT,
                    reason: "a setting holds a list",
                });
            }
            _ => return Err(r.error("a setting is a string")),
        }
    }
    Ok(out)
}

fn mapping(r: &mut Reader<'_>) -> Result<Option<String>, FormatError> {
    match r.expect_token()? {
        Token::StartArray => {}
        _ => return Err(r.error("mappings is an array")),
    }
    let mut found = None;
    loop {
        match r.expect_token()? {
            Token::EndArray => return Ok(found),
            Token::StartObject => {}
            _ => return Err(r.error("a mapping entry is an object")),
        }
        while let Some(kind) = r.next_field()? {
            if kind != DOC_TYPE || found.is_some() {
                return Err(FormatError::NotFollowable {
                    what: WHAT,
                    reason: "a mapping that is not one `_doc` type",
                });
            }
            let raw = r.raw_object("a mapping is an object")?;
            let text =
                std::str::from_utf8(raw).map_err(|_| r.error("a mapping that is not UTF-8"))?;
            // ALLOC: one mapping, bounded by the decoded blob.
            found = Some(text.to_owned());
        }
    }
}

fn aliases(r: &mut Reader<'_>) -> Result<Vec<String>, FormatError> {
    r.start_object("aliases is an object")?;
    // ALLOC: one name an alias, at most MAX_ALIASES.
    let mut out = Vec::new();
    while let Some(name) = r.next_field()? {
        if out.len() >= MAX_ALIASES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_ALIASES as u64,
                actual: (MAX_ALIASES as u64).saturating_add(1),
            });
        }
        out.push(name.into_owned());
        r.skip_value()?;
    }
    Ok(out)
}
