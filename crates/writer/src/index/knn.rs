//! The engine of a shard whose mapping has `knn_vector` fields: the k-NN
//! plugin's codec over the default one, with the vector fields listed as
//! derived, so the plugin and the reader here put a vector back into
//! `_source` (`KNN1040Codec`, `KNN10010DerivedSourceStoredFieldsFormat`).

use std::sync::Arc;

use lucene_codecs::codec::{DefaultLucene104Codec, Lucene104Codec, NamedLucene104Codec};
use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::derived_source::{DERIVED_VECTOR_FIELDS, NESTED_DERIVED_VECTOR_FIELDS};
use lucene_store::Directory;

use crate::mapping::Mapping;

/// The codec name a k-NN index's segments record.
pub const KNN_CODEC: &str = "KNN1040Codec";
/// The `.si` attribute naming the codec whose stored fields a k-NN segment
/// holds.
pub const KNN_DELEGATE_CODEC_ATTRIBUTE: &str = "knn_delegate_stored_fields_codec_key";

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// A shard's index writer over `dir`, taking explicit documents, and whether
/// the documents it is handed must carry a derived `_source`: true when
/// `mapping` has a `knn_vector` field, whose writer records the k-NN codec
/// and lists those fields in every segment it flushes or merges.
///
/// # Errors
///
/// The engine's message when the writer does not open.
pub fn open_writer<'d>(
    dir: &'d dyn Directory,
    mapping: &Mapping,
) -> Result<(IndexWriter<'d>, bool), String> {
    // ALLOC: one name a vector field, once a shard open.
    let vectors = mapping.knn_fields();
    let knn = !vectors.is_empty();
    let name = if knn { KNN_CODEC } else { "Lucene104" };
    // ALLOC: the engine's writer, once a shard open; its message on a failed open.
    let mut writer =
        IndexWriter::open(dir, Vec::new(), name, VERSION).map_err(|e| e.to_string())?;
    if knn {
        // ALLOC: one codec, once a shard open.
        let delegate: Arc<dyn Lucene104Codec> = Arc::new(DefaultLucene104Codec);
        writer.set_segment_attribute(KNN_DELEGATE_CODEC_ATTRIBUTE, delegate.name());
        // ALLOC: one codec, once a shard open; its message on a failure.
        let codec = NamedLucene104Codec::new(KNN_CODEC, delegate);
        writer
            .set_codec(Some(Arc::new(codec)))
            // ALLOC: error path; one message.
            .map_err(|e| e.to_string())?;
        // ALLOC: two attribute names, once a shard open.
        writer.set_merge_union_attributes(vec![
            DERIVED_VECTOR_FIELDS.to_owned(),
            NESTED_DERIVED_VECTOR_FIELDS.to_owned(),
        ]);
        let mut sorted = vectors;
        sorted.sort();
        for field in &sorted {
            writer.add_segment_list_value(DERIVED_VECTOR_FIELDS, field);
        }
    }
    // ALLOC: error path; one message.
    writer
        .enable_explicit_documents()
        .map_err(|e| e.to_string())?;
    Ok((writer, knn))
}
