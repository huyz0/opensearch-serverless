//! The `knn_vector` field: the k-NN plugin's mapper, for the shape this
//! writer indexes, a float vector in a Lucene HNSW graph.
//!
//! Ported from `KNNVectorFieldMapper.TypeParser`, `KNNMethodContext.parse`
//! and the Lucene engine's method validation (k-NN 3.10). The accepted
//! shape is `dimension`, an optional `data_type` of `float`, `meta`, and a
//! `method` of `hnsw` on the `lucene` engine with an optional `space_type`
//! (`l2`, `cosinesimil`, `innerproduct`) and `parameters` `m` and
//! `ef_construction`. A valid mapping outside it (a method with no engine,
//! which the plugin resolves to Faiss; a field with no method, which the
//! plugin indexes as doc values; the other engines, data types, modes and
//! compression) is refused as unsupported, so a router keeps it on a Java
//! writer.

use std::fmt::Write as _;

use super::error::MappingError;
use super::json::Json;
use super::params::parse_int;
use super::types::KNN_NO_DIMENSION;

/// The most dimensions a vector may have in either engine's library.
pub const MAX_DIMENSION: i32 = 16_000;

/// `KNNConstants.SPACE_TYPE` values, in `SpaceType`'s order.
const SPACES: &[&str] = &["l2", "cosinesimil", "l1", "linf", "innerproduct", "hamming"];

/// The spaces the Lucene engine's HNSW method supports.
pub(super) const LUCENE_SPACES: &[&str] = &["l2", "cosinesimil", "innerproduct"];

/// A `method` as `KNNMethodContext` keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct KnnMethod {
    /// The engine the mapping named, lower case; `None` when it named none.
    pub engine: Option<String>,
    /// The space type, lower case: the mapping's, or after
    /// [`resolve`] the one the field uses.
    pub space: Option<String>,
    /// The method's name.
    pub name: String,
    /// `parameters` in mapping order, or `None` for an explicit `null`.
    pub params: Option<Vec<(String, Json)>>,
}

impl KnnMethod {
    /// The parameter `key` as an `int`, once validated.
    pub fn int_param(&self, key: &str) -> Option<i32> {
        let (_, v) = self.params.as_ref()?.iter().find(|(k, _)| k == key)?;
        parse_int(v).ok()
    }

    /// The method as `KNNMethodContext.toXContent` writes it: the engine,
    /// the space type, the name and the parameters in the order Java's
    /// `HashMap` holds the two this method takes.
    pub fn write_json(&self, out: &mut String) {
        out.push_str("{\"engine\":");
        super::json::write_str(out, self.engine.as_deref().unwrap_or("lucene"));
        out.push_str(",\"space_type\":");
        super::json::write_str(out, self.space.as_deref().unwrap_or("l2"));
        out.push_str(",\"name\":");
        super::json::write_str(out, &self.name);
        out.push_str(",\"parameters\":");
        match &self.params {
            None => out.push_str("null"),
            Some(params) => {
                out.push('{');
                let mut first = true;
                // The order Java's `HashMap` holds the keys in.
                let order: &[&str] = if self.engine.as_deref() == Some("faiss") {
                    &["ef_search", "ef_construction", "m", "encoder"]
                } else {
                    &["ef_construction", "m"]
                };
                for key in order {
                    if let Some((_, v)) = params.iter().find(|(k, _)| k == key) {
                        if !first {
                            out.push(',');
                        }
                        first = false;
                        super::json::write_str(out, key);
                        let _ = write!(out, ":{}", v.to_json());
                    }
                }
                out.push('}');
            }
        }
        out.push('}');
    }
}

/// Reads `dimension` as the builder's parser does.
///
/// # Errors
///
/// `IllegalArgumentException` for a value that is no `int` or is not positive.
pub fn parse_dimension(name: &str, node: &Json) -> Result<i32, MappingError> {
    let Ok(value) = parse_int(node) else {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::illegal_argument(&format!(
            "Unable to parse [dimension] from provided value [{}] for vector [{name}]",
            node.java_string()
        )));
    };
    if value <= 0 {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::illegal_argument(&format!(
            "Dimension value must be greater than 0 for vector: {name}"
        )));
    }
    Ok(value)
}

/// Reads `data_type`: `float` is the one indexed.
///
/// # Errors
///
/// `IllegalArgumentException` for an unknown type, or unsupported for another vector type.
pub fn parse_data_type(node: &Json) -> Result<String, MappingError> {
    let text = node.java_string();
    match text.as_str() {
        "float" => Ok(text),
        "byte" | "binary" | "half_float" => {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            Err(MappingError::unsupported(&format!(
                "knn_vector data_type [{text}]"
            )))
        }
        _ => Err(MappingError::illegal_argument(
            "Invalid value provided for [data_type] field. Supported values are [binary,byte,float,half_float]",
        )),
    }
}

/// Reads the top-level `space_type` (`SpaceType.getSpace`).
///
/// # Errors
///
/// `IllegalArgumentException` for an unknown space.
pub fn parse_space(node: &Json) -> Result<String, MappingError> {
    let text = node.java_string();
    let lower = text.to_ascii_lowercase();
    if SPACES.contains(&lower.as_str()) {
        return Ok(lower);
    }
    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
    Err(MappingError::illegal_argument(&format!(
        "Unable to find space: {text} . Valid values are: [{}]",
        SPACES.join(", ")
    )))
}

/// Reads a `method` object (`KNNMethodContext.parse`).
///
/// # Errors
///
/// `MapperParsingException` with the plugin's messages, or unsupported for an engine that is not Lucene.
pub fn parse_method(node: &Json) -> Result<KnnMethod, MappingError> {
    let Json::Obj(members) = node else {
        return Err(MappingError::mapper_parsing(
            "Unable to parse mapping into KNNMethodContext. Object not of type \"Map\"",
        ));
    };
    let mut method = KnnMethod {
        engine: None,
        space: None,
        name: String::new(),
        params: Some(Vec::new()),
    };
    let mut engine_null = false;
    for (key, value) in members {
        match key.as_str() {
            "engine" => match value {
                Json::Null => engine_null = true,
                Json::Str(s) => {
                    let lower = s.to_ascii_lowercase();
                    if !matches!(lower.as_str(), "lucene" | "faiss" | "nmslib") {
                        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                        return Err(MappingError::mapper_parsing(&format!(
                            "Invalid engine: {s}"
                        )));
                    }
                    method.engine = Some(lower);
                }
                _ => {
                    return Err(MappingError::mapper_parsing("\"engine\" must be a string"));
                }
            },
            "space_type" => match value {
                Json::Str(s) => {
                    let lower = s.to_ascii_lowercase();
                    if !SPACES.contains(&lower.as_str()) {
                        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                        return Err(MappingError::mapper_parsing(&format!(
                            "Invalid space_type: {s}"
                        )));
                    }
                    method.space = Some(lower);
                }
                Json::Null => {
                    return Err(MappingError::mapper_parsing("Invalid space_type: null"));
                }
                _ => {
                    return Err(MappingError::mapper_parsing(
                        "\"space_type\" must be a string",
                    ));
                }
            },
            "name" => match value {
                Json::Str(s) => method.name.clone_from(s),
                _ => return Err(MappingError::mapper_parsing("namehas to be a string")),
            },
            "parameters" => match value {
                Json::Null => method.params = None,
                Json::Obj(m) => method.params = Some(m.clone()),
                _ => {
                    return Err(MappingError::mapper_parsing(
                        "Unable to parse parameters for main method component",
                    ));
                }
            },
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            other => {
                return Err(MappingError::mapper_parsing(&format!(
                    "Invalid parameter: {other}"
                )));
            }
        }
    }
    if method.name.is_empty() {
        return Err(MappingError::mapper_parsing("name needs to be set"));
    }
    if engine_null {
        return Err(MappingError::unsupported(
            "knn_vector method with a null engine",
        ));
    }
    Ok(method)
}

/// The checks `TypeParser.parse` makes once every parameter is read, for a
/// field named `name`: resolves the method's space type, validates the
/// method against its engine, and refuses what this writer does not index.
/// Returns the method as the field keeps it.
///
/// # Errors
///
/// The plugin's exceptions, or unsupported for a valid field it does not index.
pub fn resolve(
    name: &str,
    dimension: i32,
    method: Option<&KnnMethod>,
    top_space: &str,
) -> Result<Option<KnnMethod>, MappingError> {
    let top = (top_space != "undefined").then_some(top_space);
    let Some(method) = method else {
        // No method: the plugin resolves Faiss, whose limit applies first.
        check_dimension(dimension, "faiss")?;
        check_set(name, dimension)?;
        return Err(MappingError::unsupported(&format!(
            "knn_vector [{name}] without a method"
        )));
    };
    if let (Some(m), Some(t)) = (method.space.as_deref(), top)
        && m != t
    {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::mapper_parsing(&format!(
            "Cannot specify conflicting space types: \"[{m}]\" \"[{t}]\""
        )));
    }
    let space = method.space.as_deref().or(top).unwrap_or("l2");
    if space == "hamming" {
        return Err(MappingError::illegal_argument(
            "Space type [hamming] is not supported with [float] data type",
        ));
    }
    let mut resolved = method.clone();
    resolved.space = Some(space.to_owned());
    // Java keeps the parameters in a `HashMap`: their order means nothing.
    if let Some(params) = resolved.params.as_mut() {
        params.sort_by(|a, b| a.0.cmp(&b.0));
    }
    match method.engine.as_deref() {
        Some("lucene") => {}
        Some("faiss") => {
            let kept = super::knn_faiss::resolve(name, dimension, method, space, resolved)?;
            return Ok(Some(kept));
        }
        Some(_) => {
            return Err(MappingError::mapper_parsing(
                super::knn_faiss::NMSLIB_REFUSED,
            ));
        }
        None if method.name == "flat" => {
            return Err(MappingError::unsupported("knn_vector flat method"));
        }
        None => {
            return Err(MappingError::unsupported(
                "knn_vector method with no engine (resolved to faiss)",
            ));
        }
    }
    match method.name.as_str() {
        "hnsw" => {}
        "flat" => {
            return Err(MappingError::illegal_argument(
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
    if !LUCENE_SPACES.contains(&space) {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::validation(&format!(
            "Validation Failed: 1: \"hnsw\" with \"lucene\" configuration does not support space type: \"{space}\".;"
        )));
    }
    if let Some(params) = &method.params {
        for (key, value) in params {
            if !matches!(key.as_str(), "m" | "ef_construction") {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::validation(&format!(
                    "Validation Failed: 1: Unknown parameter '{key}' found;"
                )));
            }
            check_int_param(key, value)?;
        }
    }
    check_dimension(dimension, "lucene")?;
    check_set(name, dimension)?;
    Ok(Some(resolved))
}

pub(super) fn check_int_param(key: &str, value: &Json) -> Result<(), MappingError> {
    let integer = match value {
        Json::Num(t) => super::json::is_integral(t) && t.parse::<i32>().is_ok(),
        Json::Null => {
            return Err(MappingError::unsupported(
                "knn_vector null method parameter",
            ));
        }
        _ => false,
    };
    if !integer {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::validation(&format!(
            "Validation Failed: 1: value is not an instance of Integer for Integer parameter [{key}].;"
        )));
    }
    if parse_int(value).is_ok_and(|v| v > 0) {
        return Ok(());
    }
    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
    Err(MappingError::validation(&format!(
        "Validation Failed: 1: parameter validation failed for Integer parameter [{key}].;"
    )))
}

pub(super) fn check_dimension(dimension: i32, engine: &str) -> Result<(), MappingError> {
    if dimension > MAX_DIMENSION {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::validation(&format!(
            "Validation Failed: 1: Dimension value cannot be greater than {MAX_DIMENSION} for vector with engine: {engine};"
        )));
    }
    Ok(())
}

pub(super) fn check_set(name: &str, dimension: i32) -> Result<(), MappingError> {
    if dimension == KNN_NO_DIMENSION {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::illegal_argument(&format!(
            "Dimension value missing for vector: {name}"
        )));
    }
    Ok(())
}
