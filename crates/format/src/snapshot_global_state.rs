//! The global state a snapshot record may carry (ADR 0068): one top-level
//! `"global_state"` member holding the persistent cluster settings and five
//! sets of named JSON documents, written by a take and read by a restore.
//!
//! The member is embedded in the record so a delete stays one swap. The
//! summary readers skip it; only [`GlobalState::find`] reads it, strictly.

use crate::error::{FormatError, corrupt, len64};
use crate::json::out::Out;
use crate::json::{Reader, Token};
use crate::snapshot_pins::MAX_SNAPSHOT_RECORD_BYTES;

const WHAT: &str = "snapshot global state";
/// The key of the settings member.
/// The member as a field, which the size limit does not count.
const MEMBER: &str = "\"global_state\":";
const SETTINGS_KEY: &str = "persistent_settings";

/// The most a record's global state occupies once encoded, in bytes.
pub const MAX_GLOBAL_STATE_BYTES: usize = 4 * 1024 * 1024;

/// One of the five document sets a snapshot's global state holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateSet {
    /// Composable index templates.
    IndexTemplates,
    /// Component templates.
    ComponentTemplates,
    /// Ingest pipelines.
    IngestPipelines,
    /// Search pipelines.
    SearchPipelines,
    /// Stored scripts.
    StoredScripts,
}

impl StateSet {
    /// Every set, in the order the record writes them.
    pub const ALL: [Self; 5] = [
        Self::IndexTemplates,
        Self::ComponentTemplates,
        Self::IngestPipelines,
        Self::SearchPipelines,
        Self::StoredScripts,
    ];

    /// The member's name in the record.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::IndexTemplates => "index_templates",
            Self::ComponentTemplates => "component_templates",
            Self::IngestPipelines => "ingest_pipelines",
            Self::SearchPipelines => "search_pipelines",
            Self::StoredScripts => "stored_scripts",
        }
    }

    const fn slot(self) -> usize {
        match self {
            Self::IndexTemplates => 0,
            Self::ComponentTemplates => 1,
            Self::IngestPipelines => 2,
            Self::SearchPipelines => 3,
            Self::StoredScripts => 4,
        }
    }
}

/// A persistent setting's value: the cluster settings hold scalars only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scalar {
    /// A string.
    Str(String),
    /// An integer.
    Int(i64),
    /// Any other number, as its JSON text.
    Number(String),
    /// A boolean.
    Bool(bool),
}

/// The global state a snapshot holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalState {
    /// The persistent cluster settings, by key.
    pub settings: Vec<(String, Scalar)>,
    /// The documents of each set, by name, as the JSON object each held.
    sets: [Vec<(String, Vec<u8>)>; 5],
}

impl GlobalState {
    /// The documents of `set`.
    #[must_use]
    pub fn documents(&self, set: StateSet) -> &[(String, Vec<u8>)] {
        self.sets.get(set.slot()).map_or(&[], Vec::as_slice)
    }

    /// Adds a document to `set`; the caller passes one JSON object.
    pub fn push(&mut self, set: StateSet, name: String, document: Vec<u8>) {
        if let Some(docs) = self.sets.get_mut(set.slot()) {
            docs.push((name, document));
        }
    }

    /// Whether nothing is held: a take writes no member for it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty() && self.sets.iter().all(Vec::is_empty)
    }

    /// Writes the member's object into an open record.
    pub(crate) fn write(&self, out: &mut Out<'_>) {
        self.write_upto(out, StateSet::ALL.len());
    }

    /// Writes the settings and the first `sets` sets.
    fn write_upto(&self, out: &mut Out<'_>, sets: usize) {
        out.start_object_field("global_state");
        if !self.settings.is_empty() {
            out.start_object_field(SETTINGS_KEY);
            for (key, value) in &self.settings {
                match value {
                    Scalar::Str(s) => out.string_field(key, s),
                    Scalar::Int(n) => out.int_field(key, *n),
                    Scalar::Number(text) => out.raw_field(key, text.as_bytes()),
                    Scalar::Bool(b) => out.bool_field(key, *b),
                }
            }
            out.end_object();
        }
        for set in StateSet::ALL.into_iter().take(sets) {
            let docs = self.documents(set);
            if docs.is_empty() {
                continue;
            }
            out.start_object_field(set.key());
            for (name, doc) in docs {
                out.raw_field(name, doc);
            }
            out.end_object();
        }
        out.end_object();
    }

    /// Checks that the encoded member is at most [`MAX_GLOBAL_STATE_BYTES`].
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] whose `what` names the first set (or the
    /// settings) that pushed it over.
    pub fn check_size(&self) -> Result<(), FormatError> {
        for sets in 0..=StateSet::ALL.len() {
            // Nothing is written: an empty buffer only counts.
            let mut out = Out::new(&mut []);
            self.write_upto(&mut out, sets);
            // The limit is the member object, not its field name.
            let needed = out.len().saturating_sub(MEMBER.len());
            if needed > MAX_GLOBAL_STATE_BYTES {
                let what = match sets.checked_sub(1).and_then(|n| StateSet::ALL.get(n)) {
                    Some(set) => set.key(),
                    None => SETTINGS_KEY,
                };
                return Err(FormatError::TooLarge {
                    what,
                    limit: len64(MAX_GLOBAL_STATE_BYTES),
                    actual: len64(needed),
                });
            }
        }
        Ok(())
    }

    /// Reads a record's global state, strictly; `None` when it has none.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a member that is not an object of the
    /// shape a take writes, a document that is not an object,
    /// a setting that is not a scalar or a name written twice;
    /// [`FormatError::TooLarge`] past the record or state limit.
    pub fn find(record: &[u8]) -> Result<Option<Self>, FormatError> {
        if record.len() > MAX_SNAPSHOT_RECORD_BYTES {
            return Err(corrupt(WHAT, "a record past the size limit"));
        }
        let mut json = Reader::new(record, WHAT);
        json.start_object("a snapshot record is an object")?;
        let mut found = None;
        while let Some(field) = json.next_field()? {
            if field == "global_state" {
                let raw = json.raw_object("global state is an object")?;
                if raw.len() > MAX_GLOBAL_STATE_BYTES {
                    return Err(FormatError::TooLarge {
                        what: WHAT,
                        limit: len64(MAX_GLOBAL_STATE_BYTES),
                        actual: len64(raw.len()),
                    });
                }
                found = Some(read_state(raw)?);
            } else {
                json.skip_value()?;
            }
        }
        json.finish()?;
        Ok(found)
    }
}

fn read_state(raw: &[u8]) -> Result<GlobalState, FormatError> {
    let mut json = Reader::new(raw, WHAT);
    json.start_object("global state is an object")?;
    let mut state = GlobalState::default();
    let mut seen = [false; 6];
    while let Some(key) = json.next_field()? {
        let slot = if key == SETTINGS_KEY {
            Some(5)
        } else {
            StateSet::ALL.iter().position(|s| s.key() == key)
        };
        let Some(slot) = slot else {
            json.skip_value()?;
            continue;
        };
        if std::mem::replace(
            seen.get_mut(slot).ok_or_else(|| json.error("no slot"))?,
            true,
        ) {
            return Err(json.error("a global state member is written twice"));
        }
        if slot == 5 {
            read_settings(&mut json, &mut state.settings)?;
        } else if let Some(docs) = state.sets.get_mut(slot) {
            read_documents(&mut json, docs)?;
        }
    }
    json.finish()?;
    Ok(state)
}

fn read_settings(
    json: &mut Reader<'_>,
    out: &mut Vec<(String, Scalar)>,
) -> Result<(), FormatError> {
    json.start_object("persistent settings are an object")?;
    while let Some(key) = json.next_field()? {
        if out.iter().any(|(k, _)| *k == key) {
            return Err(json.error("a persistent setting is written twice"));
        }
        let value = match json.expect_token()? {
            // ALLOC: one value, bounded by the state size.
            Token::Str(s) => Scalar::Str(s.into_owned()),
            Token::Int(n) => Scalar::Int(n),
            Token::OtherNumber => Scalar::Number(json.number_text().to_owned()),
            Token::Bool(b) => Scalar::Bool(b),
            _ => return Err(json.error("a persistent setting is a scalar")),
        };
        // ALLOC: one key, bounded by the state size.
        out.push((key.into_owned(), value));
    }
    Ok(())
}

fn read_documents(
    json: &mut Reader<'_>,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), FormatError> {
    json.start_object("a document set is an object")?;
    while let Some(name) = json.next_field()? {
        if out.iter().any(|(n, _)| *n == name) {
            return Err(json.error("a document is written twice"));
        }
        let doc = json.raw_object("a document is an object")?;
        // ALLOC: one document, bounded by the state size.
        out.push((name.into_owned(), doc.to_vec()));
    }
    Ok(())
}
