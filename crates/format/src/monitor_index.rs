//! The alerting monitor index: one register a tenant that names each of its
//! monitors, with what a scheduler needs without reading a monitor's body.
//!
//! The marker is `{"version":n,"monitors":{id:{"c":content,"e":enabled,
//! "s":schedule,"u":updated_ms}}}`, ids in byte order. A monitor's body is
//! an immutable content blob (`blob-<content>`) beside it, as a document
//! set's is; the index is what makes a put, a delete and an enable or
//! disable one compare-and-swap, and what a runner reads once to know every
//! enabled monitor's schedule. New in this repository: the Java shell has no
//! alerting, so there are no Java-written bytes to read.

use std::collections::BTreeMap;

use crate::error::{FormatError, corrupt};
use crate::json::out::Out;
use crate::json::{Reader, Token};
use crate::schedule_registry::check_tenant_id;

const WHAT: &str = "alerting monitor index";

/// The most monitors one tenant holds.
pub const MAX_MONITORS: usize = 1000;

/// The longest monitor id, in UTF-8 bytes.
pub const MAX_MONITOR_ID_BYTES: usize = 255;

/// The longest schedule text.
pub const MAX_SCHEDULE_BYTES: usize = 256;

/// The longest monitor body.
pub const MAX_MONITOR_BYTES: usize = 1 << 20;

/// The longest index: [`MAX_MONITORS`] entries of the longest id, schedule
/// and a content key, with the field names, quotes and numbers.
pub const MAX_MONITOR_INDEX_BYTES: usize = MAX_MONITORS * 768;

pub(crate) fn check_base(base: &str) -> Result<(), FormatError> {
    if base.is_empty() || base.ends_with('/') {
        Ok(())
    } else {
        Err(corrupt(WHAT, "a base is empty or ends in '/'"))
    }
}

/// The key of a tenant's monitor index register.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`, or an invalid tenant id.
pub fn monitor_index_key(base: &str, tenant: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_tenant_id(tenant)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, "alerting/", tenant, "/monitors/_index"].concat())
}

/// The key of a monitor body's content blob.
///
/// # Errors
///
/// As [`monitor_index_key`], and for a content key that is empty, a dot
/// segment, or holds `/`.
pub fn monitor_content_key(base: &str, tenant: &str, content: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_tenant_id(tenant)?;
    if content.is_empty() || content == "." || content == ".." || content.contains('/') {
        return Err(corrupt(WHAT, "a content key is a plain path segment"));
    }
    // ALLOC: one key, the length of its parts.
    Ok([base, "alerting/", tenant, "/monitors/blob-", content].concat())
}

/// One monitor's entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorEntry {
    /// The content blob that holds the monitor's body.
    pub content: String,
    /// Whether the scheduler runs it.
    pub enabled: bool,
    /// Its schedule as the runner reads it.
    pub schedule: String,
    /// When it was last changed, in milliseconds since the epoch.
    pub updated_ms: i64,
    /// How many times this monitor has been written: 1 for its first put.
    pub version: i64,
    /// The index's change count at this monitor's last write; the plugin's
    /// `_seq_no`, which an `if_seq_no` is compared with.
    pub seq_no: i64,
}

/// A tenant's monitors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MonitorIndex {
    /// Changes made, moved on by each swap.
    pub version: i64,
    entries: BTreeMap<String, MonitorEntry>,
}

pub(crate) fn check_id(id: &str) -> Result<(), FormatError> {
    if id.len() > MAX_MONITOR_ID_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_MONITOR_ID_BYTES as u64,
            actual: id.len() as u64,
        });
    }
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.contains(['/', '"', '\\'])
        || id.chars().any(char::is_control)
    {
        return Err(corrupt(
            WHAT,
            "a monitor id is a plain path segment with no quote, backslash or control character",
        ));
    }
    Ok(())
}

fn check_entry(entry: &MonitorEntry) -> Result<(), FormatError> {
    if entry.schedule.len() > MAX_SCHEDULE_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_SCHEDULE_BYTES as u64,
            actual: entry.schedule.len() as u64,
        });
    }
    // A character the writer would escape is refused, so an entry never
    // takes more than its bound in the index.
    if entry
        .schedule
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        return Err(corrupt(
            WHAT,
            "a schedule has no quote, backslash or control character",
        ));
    }
    let c = &entry.content;
    if c.is_empty() || c == "." || c == ".." || c.contains('/') {
        return Err(corrupt(WHAT, "a content key is a plain path segment"));
    }
    Ok(())
}

impl MonitorIndex {
    /// An empty index at `version`.
    pub const fn new(version: i64) -> Self {
        // ALLOC: none; an empty map holds no heap until the first insert.
        Self {
            version,
            entries: BTreeMap::new(),
        }
    }

    /// The entry of `id`.
    pub fn get(&self, id: &str) -> Option<&MonitorEntry> {
        self.entries.get(id)
    }

    /// The monitors, in byte order of their ids.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &MonitorEntry)> {
        self.entries.iter().map(|(id, e)| (id.as_str(), e))
    }

    /// How many monitors.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there is none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether any monitor is enabled.
    pub fn any_enabled(&self) -> bool {
        self.entries.values().any(|e| e.enabled)
    }

    /// Sets the entry of `id`, returning the one it replaced.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] or [`FormatError::TooLarge`] for an invalid
    /// id, schedule or content key, or a new id past [`MAX_MONITORS`].
    pub fn insert(
        &mut self,
        id: &str,
        entry: MonitorEntry,
    ) -> Result<Option<MonitorEntry>, FormatError> {
        check_id(id)?;
        check_entry(&entry)?;
        if !self.entries.contains_key(id) && self.entries.len() >= MAX_MONITORS {
            return Err(too_many(self.entries.len()));
        }
        // ALLOC: one id, at most MAX_MONITORS kept.
        Ok(self.entries.insert(id.to_owned(), entry))
    }

    /// Removes the entry of `id`.
    pub fn remove(&mut self, id: &str) -> Option<MonitorEntry> {
        self.entries.remove(id)
    }

    /// Whether any entry reads `content`.
    pub fn shares(&self, content: &str) -> bool {
        self.entries.values().any(|e| e.content == content)
    }

    /// Reads an index.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for anything the writer does not write (a
    /// repeated or missing field, a wrong type, an invalid id or content
    /// key); [`FormatError::TooLarge`] past the bounds.
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.len() > MAX_MONITOR_INDEX_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_MONITOR_INDEX_BYTES as u64,
                actual: value.len() as u64,
            });
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("an index is an object")?;
        let mut index = Self::new(0);
        let (mut saw_version, mut saw_monitors) = (false, false);
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "version" => {
                    if std::mem::replace(&mut saw_version, true) {
                        return Err(json.error("a field is repeated"));
                    }
                    let Token::Int(n) = json.expect_token()? else {
                        return Err(json.error("version is an integer"));
                    };
                    index.version = n;
                }
                "monitors" => {
                    if std::mem::replace(&mut saw_monitors, true) {
                        return Err(json.error("a field is repeated"));
                    }
                    json.start_object("monitors is an object")?;
                    while let Some(id) = json.next_field()? {
                        let entry = Self::parse_entry(&mut json)?;
                        if index.entries.contains_key(id.as_ref()) {
                            return Err(json.error("a monitor id is repeated"));
                        }
                        index.insert(&id, entry)?;
                    }
                }
                _ => return Err(json.error("an unknown field")),
            }
        }
        json.finish()?;
        if !saw_version || !saw_monitors {
            return Err(corrupt(WHAT, "version and monitors are both present"));
        }
        Ok(index)
    }

    fn parse_entry(json: &mut Reader<'_>) -> Result<MonitorEntry, FormatError> {
        json.start_object("a monitor is an object")?;
        let (mut content, mut enabled, mut schedule, mut updated) = (None, None, None, None);
        let (mut version, mut seq_no) = (None, None);
        while let Some(field) = json.next_field()? {
            match (field.as_ref(), json.expect_token()?) {
                ("c", Token::Str(s)) if content.is_none() => content = Some(s.into_owned()),
                ("e", Token::Bool(b)) if enabled.is_none() => enabled = Some(b),
                ("s", Token::Str(s)) if schedule.is_none() => schedule = Some(s.into_owned()),
                ("u", Token::Int(n)) if updated.is_none() => updated = Some(n),
                ("v", Token::Int(n)) if version.is_none() => version = Some(n),
                ("q", Token::Int(n)) if seq_no.is_none() => seq_no = Some(n),
                _ => return Err(json.error("a monitor holds c, e, s, u, v and q once each")),
            }
        }
        match (content, enabled, schedule, updated, version, seq_no) {
            (
                Some(content),
                Some(enabled),
                Some(schedule),
                Some(updated_ms),
                Some(version),
                Some(seq_no),
            ) => Ok(MonitorEntry {
                content,
                enabled,
                schedule,
                updated_ms,
                version,
                seq_no,
            }),
            _ => Err(corrupt(WHAT, "a monitor holds c, e, s, u, v and q")),
        }
    }

    /// Writes the index, ids in byte order, into the front of `buf`.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] for a `buf` shorter than the index;
    /// [`MAX_MONITOR_INDEX_BYTES`] suffices.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        out.int_field("version", self.version);
        out.start_object_field("monitors");
        for (id, e) in &self.entries {
            out.start_object_field(id);
            out.string_field("c", &e.content);
            out.bool_field("e", e.enabled);
            out.string_field("s", &e.schedule);
            out.int_field("u", e.updated_ms);
            out.int_field("v", e.version);
            out.int_field("q", e.seq_no);
            out.end_object();
        }
        out.end_object();
        out.end_object();
        out.finish(WHAT)
    }
}

fn too_many(have: usize) -> FormatError {
    FormatError::TooLarge {
        what: WHAT,
        limit: MAX_MONITORS as u64,
        actual: (have as u64).saturating_add(1),
    }
}
