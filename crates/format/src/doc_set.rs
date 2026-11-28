//! Document sets: index and component templates, ingest and search
//! pipelines. A set is operator-authored configuration, a few documents read
//! by name, kept as immutable content blobs named by one marker register.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/template/TemplateStore.java`
//! (`readMarker`, `writeMarker`, `CONTENT_PREFIX`, `VERSION_BLOB`) and
//! `metadata/RegisterMap.java` (`indexTemplates`, `componentTemplates`,
//! `pipelines`, `searchPipelines`). The marker is `{"version":n,"entries":
//! {name: content}}`, entries in `String` order; a document's bytes are the
//! blob `blob-<content>` beside it.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt};
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "document set marker";

/// The most documents one set holds: `TemplateStore.MAX_TEMPLATES`.
pub const MAX_DOCS: usize = 1000;

/// The longest document name, in UTF-8 bytes: an index name's limit.
pub const MAX_DOC_NAME_BYTES: usize = 255;

/// The longest document this reader holds. The Java bounds it only at the
/// REST edge; a bound here keeps a read in one buffer.
pub const MAX_DOC_BYTES: usize = 1 << 20;

/// The longest model a learning to rank store holds: a tree ensemble of a few
/// thousand trees is some megabytes of JSON or XML.
pub const MAX_MODEL_BYTES: usize = 10 << 20;

/// The longest marker: [`MAX_DOCS`] entries of a longest name and a content
/// key of 64 hex digits, `-` and a 22-character id (the blob is then
/// `blob-` and the key), with the quotes, colon and comma.
pub const MAX_DOC_SET_MARKER_BYTES: usize = 384 * 1024;

/// The marker register's name in a set's container (`VERSION_BLOB`).
const MARKER: &str = "_version";

/// The prefix of a document's content blob (`CONTENT_PREFIX`).
const CONTENT_PREFIX: &str = "blob-";

/// Which set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// Composable index templates, `templates/index/`.
    IndexTemplate,
    /// Component templates, `templates/component/`.
    ComponentTemplate,
    /// Ingest pipelines, `pipelines/`.
    IngestPipeline,
    /// Search pipelines, `search_pipelines/`.
    SearchPipeline,
    /// Stored scripts, `scripts/`.
    StoredScript,
    /// Security roles, `security/roles/`.
    SecurityRoles,
    /// Security role mappings, `security/rolesmapping/`.
    SecurityRolesMapping,
    /// Security internal users, `security/internalusers/`.
    SecurityInternalUsers,
    /// Security tenants, `security/tenants/`.
    SecurityTenants,
    /// Security action groups, `security/actiongroups/`.
    SecurityActionGroups,
    /// The learning to rank stores of a deployment, `ltr/stores/`.
    LtrStores,
    /// A learning to rank store's feature sets, `featuresets/` under the
    /// store's own base.
    LtrFeatureSets,
    /// A learning to rank store's models, `models/` under the store's own
    /// base.
    LtrModels,
    /// Index state management policies, `ism/policies/`.
    IsmPolicies,
    /// Anomaly detection detectors, `ad/detectors/`.
    AdDetectors,
}

impl DocKind {
    /// The set's container under a deployment's base, ending in `/`.
    pub const fn dir(self) -> &'static str {
        match self {
            Self::IndexTemplate => "templates/index/",
            Self::ComponentTemplate => "templates/component/",
            Self::IngestPipeline => "pipelines/",
            Self::SearchPipeline => "search_pipelines/",
            Self::StoredScript => "scripts/",
            Self::SecurityRoles => "security/roles/",
            Self::SecurityRolesMapping => "security/rolesmapping/",
            Self::SecurityInternalUsers => "security/internalusers/",
            Self::SecurityTenants => "security/tenants/",
            Self::SecurityActionGroups => "security/actiongroups/",
            Self::LtrStores => "ltr/stores/",
            Self::LtrFeatureSets => "featuresets/",
            Self::LtrModels => "models/",
            Self::IsmPolicies => "ism/policies/",
            Self::AdDetectors => "ad/detectors/",
        }
    }
}

impl DocKind {
    /// The longest document of this set: [`MAX_DOC_BYTES`], or for models
    /// [`MAX_MODEL_BYTES`].
    pub const fn max_doc_bytes(self) -> usize {
        match self {
            Self::LtrModels => MAX_MODEL_BYTES,
            _ => MAX_DOC_BYTES,
        }
    }
}

fn check_base(base: &str) -> Result<(), FormatError> {
    if base.is_empty() || base.ends_with('/') {
        Ok(())
    } else {
        Err(corrupt(WHAT, "a base is empty or ends in '/'"))
    }
}

fn check_content(content: &str) -> Result<(), FormatError> {
    if content.is_empty() || content == "." || content == ".." || content.contains('/') {
        return Err(corrupt(WHAT, "a content key is not a plain path segment"));
    }
    Ok(())
}

/// The key of a set's marker register (`<dir>_version`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`.
pub fn doc_set_marker_key(base: &str, kind: DocKind) -> Result<String, FormatError> {
    check_base(base)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, kind.dir(), MARKER].concat())
}

/// The key of a document's content blob (`<dir>blob-<content>`).
///
/// # Errors
///
/// As [`doc_set_marker_key`], and for a content key that is empty, a dot
/// segment, or holds `/`.
pub fn doc_content_key(base: &str, kind: DocKind, content: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_content(content)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, kind.dir(), CONTENT_PREFIX, content].concat())
}

/// A set's marker: how many changes it has had, and which content blob each
/// name reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocSetMarker<'a> {
    /// Changes made, moved on by each swap.
    pub version: i64,
    /// Name and content key, in no order until written.
    pub entries: Vec<(Cow<'a, str>, Cow<'a, str>)>,
}

impl<'a> DocSetMarker<'a> {
    /// An empty marker at `version`.
    pub const fn new(version: i64) -> Self {
        Self {
            version,
            // ALLOC: none until an entry is added.
            entries: Vec::new(),
        }
    }

    /// Names `name` as reading `content`, replacing an entry of that name.
    pub fn insert(&mut self, name: &str, content: &str) {
        self.remove(name);
        // ALLOC: one entry, at most MAX_DOCS kept.
        self.entries
            .push((Cow::Owned(name.to_owned()), Cow::Owned(content.to_owned())));
    }

    /// Removes the entry of `name`, returning its content key.
    pub fn remove(&mut self, name: &str) -> Option<Cow<'a, str>> {
        let at = self.entries.iter().position(|(n, _)| n == name)?;
        Some(self.entries.swap_remove(at).1)
    }

    /// The content key `name` reads.
    pub fn content(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| c.as_ref())
    }

    /// Whether any entry reads `content`: another name may share a blob.
    pub fn shares(&self, content: &str) -> bool {
        self.entries.iter().any(|(_, c)| c == content)
    }

    /// Parses a marker as `readMarker`: `version` defaults to 0 and
    /// `entries` to none, other fields are skipped.
    ///
    /// # Errors
    ///
    /// [`FormatError::Unsupported`] for the bare counter a store kept before
    /// its register held entries; [`FormatError::Corrupt`] for anything the
    /// Java does not write: a version that is not an integer, an entry that
    /// is not a string, a content key that is not a path segment, a repeated
    /// field; [`FormatError::TooLarge`] for more than [`MAX_DOCS`] entries.
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        if value.trim_ascii_start().first() != Some(&b'{') {
            let text = std::str::from_utf8(value).map_err(|_| corrupt(WHAT, "not UTF-8"))?;
            return match text.trim().parse::<u64>() {
                Ok(version) => Err(FormatError::Unsupported {
                    what: WHAT,
                    version,
                }),
                Err(_) => Err(corrupt(WHAT, "not a marker")),
            };
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a marker is an object")?;
        let mut marker = Self::new(0);
        let (mut saw_version, mut saw_entries) = (false, false);
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "version" => {
                    if std::mem::replace(&mut saw_version, true) {
                        return Err(json.error("a field is repeated"));
                    }
                    let Token::Int(n) = json.expect_token()? else {
                        return Err(json.error("version is an integer"));
                    };
                    marker.version = n;
                }
                "entries" => {
                    if std::mem::replace(&mut saw_entries, true) {
                        return Err(json.error("a field is repeated"));
                    }
                    json.start_object("entries is an object")?;
                    while let Some(name) = json.next_field()? {
                        let Token::Str(content) = json.expect_token()? else {
                            return Err(json.error("an entry is a string"));
                        };
                        check_content(&content)?;
                        if marker.entries.len() >= MAX_DOCS {
                            return Err(too_many(marker.entries.len()));
                        }
                        // ALLOC: one entry, at most MAX_DOCS kept.
                        marker.entries.push((name, content));
                    }
                }
                _ => json.skip_value()?,
            }
        }
        json.finish()?;
        Ok(marker)
    }

    /// Writes the marker as `writeMarker`, entries in `String` order, into
    /// the front of `buf`, and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] for more than [`MAX_DOCS`] entries or a
    /// `buf` shorter than the marker; [`MAX_DOC_SET_MARKER_BYTES`] suffices.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        if self.entries.len() > MAX_DOCS {
            return Err(too_many(self.entries.len()));
        }
        // ALLOC: one index per entry, at most MAX_DOCS, for the sort.
        let mut order: Vec<&(Cow<'a, str>, Cow<'a, str>)> = self.entries.iter().collect();
        order.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
        let mut out = Out::new(buf);
        out.start_object();
        out.int_field("version", self.version);
        out.start_object_field("entries");
        for (name, content) in order {
            out.string_field(name, content);
        }
        out.end_object();
        out.end_object();
        out.finish(WHAT)
    }
}

fn too_many(have: usize) -> FormatError {
    FormatError::TooLarge {
        what: WHAT,
        limit: MAX_DOCS as u64,
        actual: u64::try_from(have).unwrap_or(u64::MAX),
    }
}
