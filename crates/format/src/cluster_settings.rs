//! The value of the cluster config register: an operator's persistent
//! settings.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ClusterConfig.java`
//! (`parse`, `update`, `toBytes`) and the core `Settings` they call
//! (`fromXContent`, `Builder.build`'s `processLegacyLists`). The Java reads
//! a JSON object whose nested objects join their keys with `.`; a value is
//! a string, a number's text or `true`/`false`, a list of those, or `null`.
//! Building the settings then turns keys `p.0`, `p.1` ... into a list `p`,
//! and refuses `p.0` beside a `p`.
//!
//! The Java writes `Settings.toXContent`'s nested form, in a `HashMap`'s
//! order. This writes the flat form instead, as `toXContent` does with
//! `flat_settings`: one field a key, in key order. `Settings.fromXContent`
//! reads both into the same settings, so a Java node reads what this
//! writes and the reverse; only the bytes differ.

use std::collections::BTreeMap;
use std::fmt;

use crate::error::FormatError;
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "cluster settings";

/// The most settings held, measured as their flat text: every key and
/// value's bytes, plus 6 a setting and 3 a list entry for the quotes and
/// separators. Nested keys repeat in the flat form, so this bounds what a
/// read allocates whatever the value's nesting; a value past it is refused.
/// The bound is on text before escaping: written, a control character takes
/// 6 bytes, so a buffer that must hold any settings is 6 times this long.
pub const MAX_CLUSTER_SETTINGS_BYTES: usize = 256 * 1024;

/// One setting's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    /// A scalar, as text.
    Value(String),
    /// A list of scalars, as text. Only read: the Java's register holds
    /// scalars, and an update never makes a list.
    List(Vec<String>),
}

/// An operator's persistent settings, by dotted key: always settings the
/// Java's builder accepts and a Java node reads back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClusterSettings {
    settings: BTreeMap<String, Setting>,
}

/// Why an update was refused; the settings are then unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsRefused {
    /// The result would hold a list entry `p.0` beside a setting `p`, which
    /// the Java's settings builder refuses, so a Java node could no longer
    /// read the register.
    Conflict {
        /// The setting the list would take the name of.
        prefix: String,
        /// The list entry.
        key: String,
    },
    /// The changes make a list (`p.0`, `p.1` ... become one), which the
    /// Java's register refuses: it holds scalars only.
    List {
        /// The list's key.
        key: String,
    },
    /// The result would be past [`MAX_CLUSTER_SETTINGS_BYTES`].
    TooLarge {
        /// Its flat size.
        actual: usize,
    },
}

/// The register as a settings source: a scalar reads as its text, and a
/// list reads as unset, as no typed setting is a list yet.
impl oss_types::settings::Source for ClusterSettings {
    fn value(&self, key: &str) -> Option<&str> {
        match self.settings.get(key) {
            Some(Setting::Value(v)) => Some(v),
            Some(Setting::List(_)) | None => None,
        }
    }
}

impl fmt::Display for SettingsRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { prefix, key } => {
                write!(
                    f,
                    "settings can't contain values for [{prefix}] and [{key}]"
                )
            }
            Self::List { key } => write!(
                f,
                "'{key}' is a list; list-valued settings are not supported on this register"
            ),
            Self::TooLarge { actual } => write!(
                f,
                "cluster settings too large: {actual}, limit {MAX_CLUSTER_SETTINGS_BYTES}"
            ),
        }
    }
}

impl std::error::Error for SettingsRefused {}

// Settings as the Java's builder holds them: `None` is a key put as null.
type Building = BTreeMap<String, Option<Setting>>;

impl ClusterSettings {
    /// Parses the register's value as the Java's `Settings.fromXContent`:
    /// nested keys joined with `.`, numbers kept as their text, booleans as
    /// `true` or `false`, a later key replacing an earlier one, then `p.0`,
    /// `p.1` ... made a list `p` as the Java's builder does. A key set to
    /// `null` reads as absent, as the Java's `get` answers it.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for bytes that are not one JSON object, a
    /// list holding anything but scalars, or a `p.0` beside a `p`;
    /// [`FormatError::TooLarge`] for settings past
    /// [`MAX_CLUSTER_SETTINGS_BYTES`], refused before they are all read.
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        let mut json = Reader::new(value, WHAT);
        json.start_object("cluster settings are an object")?;
        // ALLOC: one entry a setting, at most MAX_CLUSTER_SETTINGS_BYTES of
        // keys and values in all.
        let mut building = Building::new();
        // ALLOC: one key at a time, as long as the value's longest key.
        let mut key = String::new();
        let mut size = 0;
        read_object(&mut json, &mut key, &mut building, &mut size)?;
        json.finish()?;
        legacy_lists(&mut building)
            .map_err(|_| json.error("a list entry `p.0` cannot be set beside a setting `p`"))?;
        Ok(Self::built(building))
    }

    /// The settings, by dotted key.
    pub fn settings(&self) -> &BTreeMap<String, Setting> {
        &self.settings
    }

    /// The setting at `key`.
    pub fn get(&self, key: &str) -> Option<&Setting> {
        self.settings.get(key)
    }

    /// Writes the settings in the flat form, one field a key in key order,
    /// into the front of `buf`, and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is shorter than the value; the
    /// error says how long it must be.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        for (key, setting) in &self.settings {
            match setting {
                Setting::Value(value) => out.string_field(key, value),
                Setting::List(list) => {
                    out.start_array_field(key);
                    for value in list {
                        out.string(value);
                    }
                    out.end_array();
                }
            }
        }
        out.end_object();
        out.finish(WHAT)
    }

    /// Applies `changes` in order, as the Java's `update` merges them: a
    /// value sets its key, `None` removes it, and the result is built as
    /// the Java's builder builds it. Lists already held are kept.
    ///
    /// # Errors
    ///
    /// [`SettingsRefused`], and the settings unchanged, when the changes
    /// make a list, as the Java's REST handler refuses, when the result
    /// would hold a `p.0` beside a `p`, as the Java's builder refuses, or
    /// when it would be past [`MAX_CLUSTER_SETTINGS_BYTES`].
    pub fn apply(&mut self, changes: &[(&str, Option<&str>)]) -> Result<(), SettingsRefused> {
        // The changes alone, as the Java's handler builds them to look for
        // a list.
        // ALLOC: one key and value a change, the operator's own, at the
        // rate of settings updates.
        let mut asked: Building = changes
            .iter()
            .map(|(key, value)| {
                // ALLOC: as above.
                let value = value.map(|v| Setting::Value(v.to_owned()));
                // ALLOC: as above.
                ((*key).to_owned(), value)
            })
            // ALLOC: as above.
            .collect();
        legacy_lists(&mut asked)?;
        if let Some((key, _)) = asked
            .iter()
            .find(|(_, setting)| matches!(setting, Some(Setting::List(_))))
        {
            // ALLOC: the error's key, once.
            return Err(SettingsRefused::List { key: key.clone() });
        }
        // ALLOC: a copy of the settings, at most MAX_CLUSTER_SETTINGS_BYTES,
        // at the rate of settings updates.
        let mut building: Building = self
            .settings
            .iter()
            .map(|(key, setting)| (key.clone(), Some(setting.clone())))
            // ALLOC: as above, one entry a setting.
            .collect();
        building.extend(asked);
        legacy_lists(&mut building)?;
        let built = Self::built(building);
        let actual = built.settings.iter().map(|(k, s)| flat_size(k, s)).sum();
        if actual > MAX_CLUSTER_SETTINGS_BYTES {
            return Err(SettingsRefused::TooLarge { actual });
        }
        *self = built;
        Ok(())
    }

    fn built(building: Building) -> Self {
        Self {
            settings: building
                .into_iter()
                .filter_map(|(key, setting)| Some((key, setting?)))
                // ALLOC: the settings' entries moved, none copied.
                .collect(),
        }
    }
}

/// A setting's share of [`MAX_CLUSTER_SETTINGS_BYTES`].
fn flat_size(key: &str, setting: &Setting) -> usize {
    let values = match setting {
        Setting::Value(value) => value.len(),
        Setting::List(list) => list.iter().map(|v| v.len().saturating_add(3)).sum(),
    };
    key.len().saturating_add(6).saturating_add(values)
}

fn read_object(
    json: &mut Reader<'_>,
    key: &mut String,
    building: &mut Building,
    size: &mut usize,
) -> Result<(), FormatError> {
    let outer = key.len();
    while let Some(field) = json.next_field()? {
        key.truncate(outer);
        key.push_str(&field);
        let setting = match json.expect_token()? {
            Token::StartObject => {
                key.push('.');
                read_object(json, key, building, size)?;
                continue;
            }
            Token::StartArray => {
                // ALLOC: one list, as long as the value holds.
                let mut list = Vec::new();
                loop {
                    match json.expect_token()? {
                        Token::EndArray => break,
                        scalar => list.push(text(json, &scalar).ok_or_else(|| {
                            json.error("only value lists are allowed in settings")
                        })?),
                    }
                }
                Some(Setting::List(list))
            }
            Token::Null => None,
            scalar => {
                Some(Setting::Value(text(json, &scalar).ok_or_else(|| {
                    json.error("a setting is a value, a list or null")
                })?))
            }
        };
        let share = setting.as_ref().map_or(key.len(), |s| flat_size(key, s));
        *size = size.saturating_add(share);
        if *size > MAX_CLUSTER_SETTINGS_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: u64::try_from(MAX_CLUSTER_SETTINGS_BYTES).unwrap_or(u64::MAX),
                actual: u64::try_from(*size).unwrap_or(u64::MAX),
            });
        }
        // ALLOC: one key a setting, within MAX_CLUSTER_SETTINGS_BYTES.
        building.insert(key.clone(), setting);
    }
    key.truncate(outer);
    Ok(())
}

/// The Java's `Settings.Builder.processLegacyLists`: for each key ending in
/// `.0`, in key order, the keys `p.0`, `p.1` ... up to the first missing or
/// null one become a list `p`, each entry as the Java's `get` answers it.
fn legacy_lists(building: &mut Building) -> Result<(), SettingsRefused> {
    // ALLOC: the keys ending in `.0`, bounded by the settings.
    let heads: Vec<String> = building
        .keys()
        .filter(|key| key.ends_with(".0"))
        .cloned()
        // ALLOC: as above.
        .collect();
    for head in heads {
        let prefix = list_of(&head);
        if building.contains_key(prefix) {
            return Err(SettingsRefused::Conflict {
                // ALLOC: the error's two keys, once.
                prefix: prefix.to_owned(),
                key: head.clone(),
            });
        }
        // ALLOC: one list, as long as the settings hold.
        let mut list = Vec::new();
        loop {
            // ALLOC: one key a list entry, bounded as above.
            let entry = format!("{prefix}.{}", list.len());
            match building.get(&entry) {
                Some(Some(setting)) => {
                    list.push(as_text(setting));
                    building.remove(&entry);
                }
                _ => break,
            }
        }
        // ALLOC: the list's key, once a list.
        building.insert(prefix.to_owned(), Some(Setting::List(list)));
    }
    // A `p.0` still beside a `p` (a null `p.0`, or a list made from
    // `p.0.0`) is settings the Java holds but cannot read back from the
    // register: its next read would refuse them.
    if let Some(key) = building
        .keys()
        .find(|key| key.ends_with(".0") && building.contains_key(list_of(key)))
    {
        // ALLOC: the error's two keys, once.
        let (prefix, key) = (list_of(key).to_owned(), key.clone());
        return Err(SettingsRefused::Conflict { prefix, key });
    }
    Ok(())
}

/// The list a key ending in `.0` is the first entry of.
fn list_of(head: &str) -> &str {
    head.get(..head.len().saturating_sub(2)).unwrap_or_default()
}

/// A setting as the Java's `get` answers it: a list as `List.toString`.
fn as_text(setting: &Setting) -> String {
    match setting {
        // ALLOC: one value, bounded by the settings.
        Setting::Value(value) => value.clone(),
        Setting::List(list) => format!("[{}]", list.join(", ")),
    }
}

/// A scalar token's text as the Java's parser answers it: a string as it
/// is, a number as written, a boolean as `true` or `false`.
fn text(json: &Reader<'_>, token: &Token<'_>) -> Option<String> {
    // ALLOC: one value, bounded by the register's value.
    match token {
        Token::Str(s) => Some(s.to_string()),
        Token::Int(_) | Token::OtherNumber => Some(json.number_text().to_owned()),
        // ALLOC: as above, one value.
        Token::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
