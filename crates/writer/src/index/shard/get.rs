//! A shard's realtime get, as `InternalEngine.get` answers it: the live
//! document of an id with its version and stored `_source`, refreshing
//! first when the id was written since the last refresh.

use lucene_codecs::stored_fields::{StoredFieldVisitor, VisitStatus};

use super::{GC_DELETES_MILLIS, Shard, lookup, uid};

/// A live document read from a shard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// `_version`.
    pub version: i64,
    /// `_seq_no`.
    pub seq_no: i64,
    /// `_primary_term`.
    pub primary_term: i64,
    /// The stored `_source`, as it was indexed.
    pub source: Vec<u8>,
}

/// Copies the `_source` field of one document.
struct Source {
    field: Option<i32>,
    bytes: Vec<u8>,
}

impl StoredFieldVisitor for Source {
    fn needs_field(&mut self, number: i32) -> lucene_codecs::stored_fields::Result<VisitStatus> {
        Ok(if Some(number) == self.field {
            VisitStatus::Yes
        } else {
            VisitStatus::No
        })
    }

    fn binary_field(
        &mut self,
        _number: i32,
        value: &[u8],
    ) -> lucene_codecs::stored_fields::Result<()> {
        // ALLOC: the document's source, one a get; bounded by the HTTP body bound it was written under.
        self.bytes = value.to_vec();
        Ok(())
    }
}

impl Shard<'_> {
    /// The live document of `id`. With `realtime`, an id written since the
    /// last refresh is made visible first, and one the version map holds as
    /// deleted is not found; without it the last refresh is read as it is.
    ///
    /// # Errors
    ///
    /// A refresh or a segment failing.
    pub fn get(&mut self, id: &str, realtime: bool) -> Result<Option<Fetched>, String> {
        if realtime {
            match self.versions.get(id, (self.clock)(), GC_DELETES_MILLIS) {
                Some(c) if c.deleted => return Ok(None),
                Some(_) => self.refresh()?,
                None => {}
            }
        }
        let Some(reader) = &self.reader else {
            return Ok(None);
        };
        // ALLOC: error path; one message.
        let key = uid(id).map_err(|e| e.to_string())?;
        let Some(found) = lookup::locate(reader, &key)? else {
            return Ok(None);
        };
        let segment = reader
            .segment_readers()
            .get(found.segment)
            // ALLOC: error path; one message.
            .ok_or_else(|| "a document outside every segment".to_owned())?;
        let mut source = Source {
            field: segment
                .field_infos()
                .field_by_name("_source")
                .map(|f| f.number),
            // ALLOC: empty until the visitor copies the one document's source.
            bytes: Vec::new(),
        };
        segment
            .visit_stored_document(found.doc, &mut source)
            // ALLOC: error path; one message.
            .map_err(|e| e.to_string())?;
        Ok(Some(Fetched {
            version: found.current.version,
            seq_no: found.current.seq_no,
            primary_term: found.current.term,
            source: source.bytes,
        }))
    }
}
