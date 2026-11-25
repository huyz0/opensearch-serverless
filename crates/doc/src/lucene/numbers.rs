//! Where a field's number comes from.

use lucene_codecs::field_infos::FieldInfo;
use lucene_codecs::per_field_knn_vectors::KnnVectorsFormat;
use lucene_index::index_writer::{Error as EngineError, IndexWriter};

/// Where a field's number comes from: the engine's field numbers, which
/// hold for every segment of an index writer and refuse a field whose
/// schema changes (`FieldInfos.FieldNumbers.addOrGet` and
/// `verifySameSchema`).
pub trait FieldNumbers {
    /// The number of the field `info` describes, registering it on first
    /// sight; `info.number` is ignored.
    ///
    /// # Errors
    ///
    /// The engine's message when the field's schema differs from the one
    /// it was registered with.
    fn number(&mut self, info: FieldInfo) -> Result<i32, String>;

    /// Routes the vector field `name` to an HNSW graph of `m` connections a
    /// node and `beam_width`, as `PerFieldKnnVectorsFormat` does.
    fn vector_graph(&mut self, _name: &str, _m: i32, _beam_width: i32) {}
}

impl FieldNumbers for IndexWriter<'_> {
    fn number(&mut self, info: FieldInfo) -> Result<i32, String> {
        self.register_field(info).map_err(|e| match e {
            EngineError::Explicit(message) => message,
            // ALLOC: error path; one message.
            other => other.to_string(),
        })
    }

    fn vector_graph(&mut self, name: &str, m: i32, beam_width: i32) {
        self.set_knn_vectors_format_for_field(
            name,
            KnnVectorsFormat::Hnsw {
                max_conn: m,
                beam_width,
            },
        );
    }
}
