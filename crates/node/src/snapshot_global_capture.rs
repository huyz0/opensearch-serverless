//! Reads the global state a take records (ADR 0068): the persistent cluster
//! settings, then each of the five document sets, one request after another
//! through three buffers, so a take that asks for it never holds more than
//! [`RESERVE_BYTES`] for it.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::{
    DocKind, GlobalState, MAX_CLUSTER_SETTINGS_BYTES, MAX_DOC_BYTES, MAX_DOC_SET_MARKER_BYTES,
    MAX_GLOBAL_STATE_BYTES, Scalar, Setting, StateSet,
};
use oss_meta::{ClusterConfig, DocSet, MetaError};

/// What a capture holds at most: the state, a document, a marker and the
/// settings' read buffer (a written setting takes up to 6 bytes a byte).
pub(crate) const RESERVE_BYTES: usize =
    MAX_GLOBAL_STATE_BYTES + MAX_DOC_BYTES + MAX_DOC_SET_MARKER_BYTES + SETTINGS_BUF;

const SETTINGS_BUF: usize = 6 * MAX_CLUSTER_SETTINGS_BYTES;

/// Why a capture stopped.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The state passed its bound; names the set (or the settings).
    TooLarge(&'static str),
    /// A set changed under the read twice.
    Changed(&'static str),
    /// The store or a record failed.
    Meta(MetaError),
}

impl From<MetaError> for Failure {
    fn from(e: MetaError) -> Self {
        Self::Meta(e)
    }
}

/// The stores a capture reads.
pub(crate) struct StateReader {
    config: ClusterConfig,
    sets: Vec<(StateSet, DocSet)>,
}

pub(crate) fn kind(set: StateSet) -> DocKind {
    match set {
        StateSet::IndexTemplates => DocKind::IndexTemplate,
        StateSet::ComponentTemplates => DocKind::ComponentTemplate,
        StateSet::IngestPipelines => DocKind::IngestPipeline,
        StateSet::SearchPipelines => DocKind::SearchPipeline,
        StateSet::StoredScripts => DocKind::StoredScript,
    }
}

impl StateReader {
    /// The reader over `store` under `base`.
    pub(crate) fn open(store: &Arc<dyn BlobStore>, base: &str) -> Result<Self, MetaError> {
        // ALLOC: five sets, once a service.
        let mut sets = Vec::with_capacity(StateSet::ALL.len());
        for set in StateSet::ALL {
            sets.push((set, DocSet::new(Arc::clone(store), base, kind(set))?));
        }
        Ok(Self {
            config: ClusterConfig::new(Arc::clone(store), base)?,
            sets,
        })
    }

    /// Reads the state: one request for the settings, then for each set its
    /// marker and each document. A set a change dropped a document from
    /// mid-walk is read again from its marker once.
    pub(crate) fn capture(&self) -> Result<GlobalState, Failure> {
        let mut state = GlobalState::default();
        // The settings are bounded by their register (256 KiB), far under the state's bound.
        let mut bytes = 0usize;
        // ALLOC: the settings read buffer, SETTINGS_BUF, reserved by the route.
        let mut settings_buf = vec![0u8; SETTINGS_BUF];
        let value = self.config.read(&mut settings_buf)?;
        for (key, setting) in value.settings.settings() {
            match setting {
                Setting::Value(v) => {
                    bytes = bytes.saturating_add(key.len()).saturating_add(v.len());
                    // ALLOC: one setting, bounded by MAX_CLUSTER_SETTINGS_BYTES.
                    state.settings.push((key.clone(), Scalar::Str(v.clone())));
                }
                Setting::List(_) => {
                    eprintln!("snapshot global state skips the list-valued setting [{key}]");
                }
            }
        }
        drop(settings_buf);
        // ALLOC: one marker buffer, MAX_DOC_SET_MARKER_BYTES, reserved by the route.
        let mut marker = vec![0u8; MAX_DOC_SET_MARKER_BYTES];
        // ALLOC: one document buffer, MAX_DOC_BYTES, reserved by the route.
        let mut body = vec![0u8; MAX_DOC_BYTES];
        for (set, docs) in &self.sets {
            let (found, used) = read_set(docs, *set, bytes, &mut marker, &mut body)?;
            bytes = bytes.saturating_add(used);
            for (name, doc) in found {
                state.push(*set, name, doc);
            }
        }
        state.check_size().map_err(|e| match e {
            oss_format::FormatError::TooLarge { what, .. } => Failure::TooLarge(what),
            other => Failure::Meta(other.into()),
        })?;
        Ok(state)
    }
}

/// A set's documents, by name.
type Documents = Vec<(String, Vec<u8>)>;

/// What one set's walk has gathered. A walk that restarts (a name no later
/// than the last) drops what it gathered.
struct Gather {
    found: Documents,
    used: usize,
    over: bool,
    last: Option<String>,
    before: usize,
}

impl Gather {
    fn new(before: usize) -> Self {
        Self {
            // ALLOC: one entry a document, at most MAX_DOCS; the running
            // bytes stop it growing past MAX_GLOBAL_STATE_BYTES.
            found: Vec::new(),
            used: 0,
            over: false,
            last: None,
            before,
        }
    }

    fn visit(&mut self, name: &str, doc: &[u8]) {
        let restarted = self
            .last
            .as_deref()
            .is_some_and(|l| l.encode_utf16().ge(name.encode_utf16()));
        if restarted {
            self.found.clear();
            self.used = 0;
            self.over = false;
        }
        // ALLOC: one name, once a document.
        self.last = Some(name.to_owned());
        if self.over || !doc.starts_with(b"{") {
            return;
        }
        self.used = self
            .used
            .saturating_add(name.len())
            .saturating_add(doc.len());
        if self.before.saturating_add(self.used) > MAX_GLOBAL_STATE_BYTES {
            self.over = true;
            self.found.clear();
            return;
        }
        // ALLOC: one document, bounded by the running total above.
        self.found.push((name.to_owned(), doc.to_vec()));
    }
}

/// One set's documents and their bytes.
fn read_set(
    docs: &DocSet,
    set: StateSet,
    before: usize,
    marker: &mut [u8],
    body: &mut [u8],
) -> Result<(Documents, usize), Failure> {
    let mut gather = Gather::new(before);
    match docs.for_each(marker, body, &mut |n, d| gather.visit(n, d)) {
        Ok(()) => {}
        Err(MetaError::MissingContent(_)) => return Err(Failure::Changed(set.key())),
        Err(e) => return Err(e.into()),
    }
    if gather.over {
        return Err(Failure::TooLarge(set.key()));
    }
    Ok((gather.found, gather.used))
}

/// The answer for a capture that stopped.
pub(crate) fn refused_state(failure: Failure) -> oss_rest::RestError {
    use crate::route_util::refuse;
    match failure {
        Failure::TooLarge(set) => refuse(
            400,
            "snapshot_too_large",
            // ALLOC: error path; one message.
            format!("the global state would pass 4 MiB at [{set}]"),
        ),
        Failure::Changed(set) => refuse(
            409,
            "snapshot_state_changed",
            // ALLOC: error path; one message.
            format!("[{set}] changed while the global state was read; try again"),
        ),
        Failure::Meta(MetaError::Blob(ref b)) => refuse(503, "metadata_unavailable", b.to_string()),
        // ALLOC: error path; one message.
        Failure::Meta(other) => refuse(500, "snapshot_failure", other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_is_answered_with_its_status_and_names_the_set() {
        let too_large = refused_state(Failure::TooLarge("stored_scripts"));
        assert_eq!(too_large.status(), 400);
        let changed = refused_state(Failure::Changed("index_templates"));
        assert_eq!(changed.status(), 409);
        let broken = refused_state(Failure::Meta(MetaError::Contended));
        assert_eq!(broken.status(), 500);
        let gone = refused_state(Failure::Meta(MetaError::Blob(
            oss_blob::BlobError::NotFound,
        )));
        assert_eq!(gone.status(), 503);
        let text = format!(
            "{:?}",
            too_large.as_exception().expect("exception").reason()
        );
        assert!(text.contains("stored_scripts"), "{text}");
        let text = format!("{:?}", changed.as_exception().expect("exception").reason());
        assert!(text.contains("index_templates"), "{text}");
    }

    #[test]
    fn a_walk_that_restarts_drops_what_it_gathered() {
        let mut g = Gather::new(0);
        g.visit("a", b"{}");
        g.visit("b", b"{}");
        g.visit("a", b"{\"n\":1}");
        assert_eq!(g.found.len(), 1);
        assert_eq!(g.found[0].1, b"{\"n\":1}");
        assert_eq!(g.used, 1 + 7);
    }

    #[test]
    fn a_name_the_same_as_the_last_restarts_and_a_later_one_does_not() {
        let mut g = Gather::new(0);
        g.visit("a", b"{}");
        g.visit("a", b"{}");
        assert_eq!(g.found.len(), 1);
        g.visit("b", b"{}");
        assert_eq!(g.found.len(), 2);
    }

    #[test]
    fn the_bytes_up_to_the_limit_are_kept_and_one_more_is_over() {
        let doc = |n: usize| {
            let mut d = b"{\"a\":\"".to_vec();
            d.resize(n - 2, b'x');
            d.extend_from_slice(b"\"}");
            d
        };
        // name "a" is 1 byte: 1 + (limit - 1) bytes in all is the limit.
        let mut g = Gather::new(0);
        g.visit("a", &doc(MAX_GLOBAL_STATE_BYTES - 1));
        assert!(!g.over);
        assert_eq!(g.found.len(), 1);
        let mut g = Gather::new(0);
        g.visit("a", &doc(MAX_GLOBAL_STATE_BYTES));
        assert!(g.over && g.found.is_empty());
        let mut g = Gather::new(10);
        g.visit("a", &doc(MAX_GLOBAL_STATE_BYTES - 10));
        assert!(g.over, "the bytes before the set count");
    }

    #[test]
    fn a_document_that_is_not_an_object_is_left_out() {
        let mut g = Gather::new(0);
        g.visit("a", b"[1]");
        assert!(g.found.is_empty());
        assert_eq!(g.used, 0);
    }
}
