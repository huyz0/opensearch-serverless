//! The `faiss` engine's float HNSW method, as the k-NN plugin's mapper
//! validates it (3.10). The mapping keeps the engine name the client wrote;
//! the segment is the Lucene HNSW one (docs/adr/0086), so a method Lucene HNSW
//! cannot honour is refused here, never approximated.

use super::error::MappingError;
use super::json::Json;
use super::knn::{KnnMethod, LUCENE_SPACES, check_dimension, check_int_param, check_set};

/// What the plugin answers for `engine: nmslib` on a new index.
pub(super) const NMSLIB_REFUSED: &str = "nmslib engine is deprecated in OpenSearch  and cannot be used for new index creation in OpenSearch from  3.0.0.";

/// Faiss HNSW takes the spaces Lucene HNSW takes.
const FAISS_SPACES: &[&str] = LUCENE_SPACES;

/// Validates `method` (already space-resolved) on the `faiss` engine and
/// returns the method the field keeps.
///
/// # Errors
///
/// The plugin's exceptions for what it refuses on Faiss, among them a method
/// that needs training.
pub(super) fn resolve(
    name: &str,
    dimension: i32,
    method: &KnnMethod,
    space: &str,
    mut resolved: KnnMethod,
) -> Result<KnnMethod, MappingError> {
    match method.name.as_str() {
        "hnsw" => {}
        "ivf" => return Err(needs_training()),
        "flat" => {
            return Err(MappingError::mapper_parsing(
                "[flat] method does not support [engine] parameter",
            ));
        }
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        other => {
            return Err(MappingError::illegal_argument(&format!(
                "Invalid method name: {other}"
            )));
        }
    }
    if !FAISS_SPACES.contains(&space) {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::validation(&format!(
            "Validation Failed: 1: \"hnsw\" with \"faiss\" configuration does not support space type: \"{space}\".;"
        )));
    }
    if let (Some(params), Some(out)) = (&method.params, resolved.params.as_mut()) {
        for (key, value) in params {
            match key.as_str() {
                "m" | "ef_construction" | "ef_search" => check_int_param(key, value)?,
                "encoder" => {
                    let kept = encoder(value)?;
                    if let Some(slot) = out.iter_mut().find(|(k, _)| k == "encoder") {
                        slot.1 = kept;
                    }
                }
                other => {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::validation(&format!(
                        "Validation Failed: 1: Unknown parameter '{other}' found;"
                    )));
                }
            }
        }
    }
    check_dimension(dimension, "faiss")?;
    check_set(name, dimension)?;
    Ok(resolved)
}

fn needs_training() -> MappingError {
    MappingError::validation("Validation Failed: 1: \"method\" requires training.;")
}

/// An `encoder` the HNSW method takes without training, normalised to the
/// `{"name": .., "parameters": {}}` the plugin writes back.
fn encoder(value: &Json) -> Result<Json, MappingError> {
    let name = match value.get("name") {
        Some(Json::Str(n)) => n.as_str(),
        _ => return Err(MappingError::mapper_parsing("name needs to be set")),
    };
    match name {
        "flat" | "binary" => {
            let extra = match value.get("parameters") {
                Some(Json::Obj(m)) => m.first().map(|(k, _)| k.clone()),
                _ => None,
            };
            if let Some(key) = extra {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::validation(&format!(
                    "Validation Failed: 1: Unknown parameter '{key}' found;"
                )));
            }
            Ok(Json::Obj(vec![
                ("name".to_owned(), Json::Str(name.to_owned())),
                ("parameters".to_owned(), Json::Obj(Vec::new())),
            ]))
        }
        "pq" => Err(needs_training()),
        "sq" => Err(MappingError::validation(
            "Validation Failed: 1: Parameter [bits] is required for encoder [sq] on indices created with version 3.6.0 or later. Supported values: [2, 1, 16, 4];",
        )),
        _ => Err(MappingError::null_pointer(
            "Cannot invoke \"org.opensearch.knn.index.engine.MethodComponent.isTrainingRequired(org.opensearch.knn.index.engine.MethodComponentContext)\" because \"methodComponent\" is null",
        )),
    }
}
