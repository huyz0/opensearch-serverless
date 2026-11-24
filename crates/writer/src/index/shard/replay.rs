//! A shard applying a record of a predecessor's write log, as
//! `InternalEngine` applies an operation recovered from a translog: with the
//! sequence number, term and version the record carries.

use lucene_index::buffered_updates::Term;
use oss_format::WalRecord;

use super::{Current, Refused, Shard, document, soft_delete, uid};
use crate::lucene::Meta;
use crate::mapping::MappingError;

impl Shard<'_> {
    /// Applies `record` as the predecessor wrote it, keeping its sequence
    /// number, term and version; one with no sequence identity takes the
    /// next sequence number and version under this shard's term. A record
    /// at or below the sequence number the id already holds is stale and
    /// changes nothing and answers `false`, so a replay over a commit that holds some of its
    /// operations is safe. Takes no conflict check: the predecessor's
    /// writes were checked when it accepted them.
    ///
    /// # Errors
    ///
    /// The document or the mapping refusing it, or the engine failing.
    pub fn replay(&mut self, record: &WalRecord<'_>) -> Result<bool, Refused> {
        let id = record.id();
        let key = uid(id).map_err(Refused::Mapping)?;
        let current = self.current(id, &key)?;
        let sequenced = record.has_sequence_identity();
        if sequenced && current.is_some_and(|c| c.seq_no >= record.seq_no()) {
            // Stale or already held: only the high-water mark may move.
            self.max_seq_no = self.max_seq_no.max(record.seq_no());
            return Ok(false);
        }
        let seq_no = if sequenced {
            record.seq_no()
        } else {
            self.max_seq_no.saturating_add(1)
        };
        let primary_term = if sequenced {
            record.primary_term()
        } else {
            self.primary_term
        };
        let version = if sequenced {
            record.version()
        } else {
            current.map_or(1, |c| c.version.saturating_add(1))
        };
        let live = current.is_some_and(|c| !c.deleted);
        if record.is_deletion() {
            if live {
                self.indexer
                    .soft_deletes(&mut self.writer)
                    .map_err(Refused::Mapping)?;
            }
            let tombstone = self
                .indexer
                .tombstone(&mut self.writer, id, seq_no, primary_term, version)
                .map_err(Refused::Mapping)?;
            // ALLOC: one tombstone a replayed delete.
            let docs = vec![tombstone];
            if live {
                self.writer.soft_update_explicit_documents(
                    Term::new("_id", key.clone()),
                    docs,
                    &[soft_delete(&key)],
                )
            } else {
                self.writer.add_explicit_documents(docs)
            }
            .map_err(|e| Self::engine(&e))?;
        } else {
            let source = record.source().as_bytes();
            let routing = record.routing();
            let fast = if routing.is_some() {
                None
            } else {
                self.fast
                    .prepare(&self.indexer, &self.mapping, source, &key)
            };
            let parsed = match fast {
                Some(_) => None,
                None => Some(document::parse(&self.mapping, id, source).map_err(Refused::Mapping)?),
            };
            if let Some(grown) = parsed.as_ref().and_then(|p| p.grown.as_ref()) {
                // ALLOC: the grown mapping, bounded by the total fields limit.
                self.mapping = grown.clone();
                self.fast.invalidate();
            }
            let soft = if live {
                Some(
                    self.indexer
                        .soft_deletes(&mut self.writer)
                        .map_err(Refused::Mapping)?,
                )
            } else {
                None
            };
            let meta = Meta {
                id,
                seq_no,
                primary_term,
                version,
                source,
            };
            let (doc, vectors) = match (fast, &parsed) {
                (Some(f), _) => (
                    f.finish(seq_no, primary_term, version)
                        .pop()
                        .ok_or_else(|| {
                            Refused::Mapping(MappingError::illegal_state("no document read"))
                        })?,
                    Vec::new(),
                ),
                (None, Some(parsed)) => self
                    .indexer
                    .document_with_vectors(&mut self.writer, &self.mapping, parsed, &meta, routing)
                    .map_err(Refused::Mapping)?,
                (None, None) => {
                    return Err(Refused::Mapping(MappingError::illegal_state(
                        "no document read",
                    )));
                }
            };
            // ALLOC: one document a replayed index.
            let docs = vec![doc];
            match (soft, vectors.is_empty()) {
                (None, true) => self.writer.add_explicit_documents(docs),
                (None, false) => {
                    self.writer
                        .add_explicit_documents_with_vectors(None, docs, vec![vectors])
                }
                (Some(_), true) => self.writer.soft_update_explicit_documents(
                    Term::new("_id", key.clone()),
                    docs,
                    &[soft_delete(&key)],
                ),
                (Some(_), false) => self.writer.soft_update_explicit_documents_with_vectors(
                    Term::new("_id", key.clone()),
                    docs,
                    vec![vectors],
                    &[soft_delete(&key)],
                ),
            }
            .map_err(|e| Self::engine(&e))?;
        }
        self.max_seq_no = self.max_seq_no.max(seq_no);
        let now = (self.clock)();
        self.versions.put(
            id,
            Current {
                version,
                seq_no,
                term: primary_term,
                deleted: record.is_deletion(),
            },
            now,
        );
        self.last_write_millis = now;
        Ok(true)
    }
}
