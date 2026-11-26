//! A source read straight into Lucene documents by lucene-rust's source ingest, in front of
//! [`crate::document::parse`] and [`crate::lucene::Indexer`].
//!
//! The ingest reads a source against a plan of the mapping's fields. The plan names only the
//! fields the shard's `Indexer` has already numbered, with those numbers, so the engine's field
//! numbering stays in the order a document first shows each field. Anything the plan does not
//! reproduce digit for digit (an unmapped or not yet numbered field, a repeated field, a
//! coercion, a source that is not strict JSON) is declined, and the caller takes the complete
//! path, which is also the oracle the differential tests compare against.

mod plan;

use std::sync::Arc;

use lucene_engine::ingest::{self, Plan, Prepared};
use lucene_index::index_writer::ExplicitDocument;

use crate::lucene::Indexer;
use crate::mapping::Mapping;

/// Plans the registry keeps for the life of the process (`ingest`'s own cap is 16,384): past
/// this many compiled by one shard the fast path stops compiling and declines.
const MAX_COMPILES: u32 = 4096;

/// Sources over this many bytes stay with the complete path. At 1, 8 and 32 MB of text the plan
/// builds the document in the same time, and holds about twice the source's bytes more at its
/// peak (provisional; `tests/fast_large_source.rs`), so a large source gains nothing from it.
const MAX_SOURCE: usize = 1 << 20;

/// A shard's fast path: the plan compiled for its mapping and the fields numbered so far.
#[derive(Default)]
pub struct FastIngest {
    plan: Option<Arc<Plan>>,
    stale: bool,
    compiles: u32,
    /// How many fields the indexer had numbered when the plan was compiled.
    numbered: usize,
    /// Whether the mapping the plan was compiled for has a nested object, which the plan does
    /// not read as child documents: every source takes the complete path.
    nested: bool,
}

/// A source read by the plan: the documents but for their sequence number, primary term and version.
pub struct Fast {
    prepared: Prepared,
    plan: Arc<Plan>,
}

impl Fast {
    /// The documents, with the sequence number, primary term and version the engine assigned.
    pub fn finish(self, seq_no: i64, primary_term: i64, version: i64) -> Vec<ExplicitDocument> {
        let mut docs = self
            .prepared
            .finish(&self.plan, seq_no, primary_term, version);
        for doc in &mut docs {
            leave_empty_fields_unwritten(doc);
        }
        docs
    }

    /// A measure of the bytes the documents hold, for memory accounting.
    pub fn weight(&self) -> usize {
        self.prepared.weight
    }
}

impl FastIngest {
    /// A fast path with no plan yet; the first source compiles one.
    pub fn new() -> Self {
        FastIngest {
            plan: None,
            stale: true,
            compiles: 0,
            numbered: 0,
            nested: false,
        }
    }

    /// How many plans this fast path has compiled.
    pub fn compiles(&self) -> u32 {
        self.compiles
    }

    /// The mapping changed: recompile before the next source. (A field numbered since the plan
    /// was compiled is noticed without this.)
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// `source`, the document whose encoded `_id` term is `key`, read against `mapping`, or
    /// `None` when the complete path has to take it.
    pub fn prepare(
        &mut self,
        indexer: &Indexer,
        mapping: &Mapping,
        source: &[u8],
        key: &[u8],
    ) -> Option<Fast> {
        if source.len() > MAX_SOURCE {
            return None;
        }
        if self.stale || self.numbered != indexer.numbered_len() {
            if self.compiles >= MAX_COMPILES {
                return None;
            }
            self.compiles = self.compiles.saturating_add(1);
            self.nested = mapping.has_nested();
            self.plan = plan::compile(indexer, mapping);
            self.numbered = indexer.numbered_len();
            self.stale = false;
        }
        if self.nested {
            return None;
        }
        let plan = self.plan.as_ref()?;
        let prepared = ingest::prepare(plan, source, key).ok().flatten()?;
        Some(Fast {
            prepared,
            plan: Arc::clone(plan),
        })
    }
}

/// A field with no token in the document (an empty text) is one of the segment's fields with no
/// postings and no norm, as [`crate::lucene::Indexer`] gives it: its number goes to `registered`.
fn leave_empty_fields_unwritten(doc: &mut ExplicitDocument) {
    if !doc.fields.inverted.iter().any(|f| f.terms.is_empty()) {
        return;
    }
    let (kept, empty): (Vec<_>, Vec<_>) = std::mem::take(&mut doc.fields.inverted)
        .into_iter()
        .partition(|f| !f.terms.is_empty());
    doc.fields.inverted = kept;
    doc.fields
        .registered
        .extend(empty.into_iter().map(|f| f.field_number));
}
