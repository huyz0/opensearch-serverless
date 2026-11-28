//! The body of `POST /_plugins/_knn/models/{id}/_train`, validated as the
//! k-NN plugin's `TrainingModelRequest` and the Faiss method components
//! validate it (3.10), before any index is read.
//!
//! Only what the body itself decides is checked here: the fields it must
//! carry, the method's name, engine, space and parameters, and the encoder.
//! Whether the training index, its field and the dimension exist is the
//! service's, which reads the mapping.

use oss_rest::RestError;
use serde_json::{Map, Value};

use crate::route_util::refuse;

/// The vectors a training run reads at most, unless the request says less.
pub(crate) const DEFAULT_MAX_TRAINING_VECTORS: u64 = 10_000_000;

/// The product-quantization encoder of a method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pq {
    /// Sub-vectors a vector is split into.
    pub m: u32,
    /// Bits a sub-vector's code takes.
    pub code_size: u32,
}

/// A validated training request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrainRequest {
    pub training_index: String,
    pub training_field: String,
    pub dimension: u32,
    /// `l2`, `innerproduct` or `cosinesimil`.
    pub space: String,
    /// IVF's number of clusters.
    pub nlist: u32,
    /// IVF's clusters probed by default.
    pub nprobes: u32,
    pub pq: Option<Pq>,
    pub description: String,
    pub max_training_vectors: u64,
    pub search_size: u32,
}

fn illegal(reason: &str) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn action_invalid(reason: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        400,
        "action_request_validation_exception",
        format!("Validation Failed: 1: {reason};"),
    )
}

fn invalid(reason: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        400,
        "validation_exception",
        format!("Validation Failed: 1: {reason};"),
    )
}

fn text<'a>(body: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

/// A positive integer parameter, as the plugin's `Parameter.IntegerParameter` reads it.
fn positive(params: &Map<String, Value>, key: &str, default: u32) -> Result<u32, RestError> {
    let Some(v) = params.get(key) else {
        return Ok(default);
    };
    let Some(n) = v.as_i64() else {
        return Err(action_invalid(&format!(
            "value is not an instance of Integer for Integer parameter [{key}]."
        )));
    };
    match u32::try_from(n) {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(action_invalid(&format!(
            "parameter validation failed for Integer parameter [{key}]."
        ))),
    }
}

fn pq_of(encoder: &Value, dimension: u32) -> Result<Option<Pq>, RestError> {
    let name = encoder.get("name").and_then(Value::as_str).unwrap_or("");
    match name {
        "flat" => Ok(None),
        "pq" => {
            let empty = Map::new();
            let p = encoder
                .get("parameters")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            let m = positive(p, "m", 1)?;
            let code_size = positive(p, "code_size", 8)?;
            if !dimension.is_multiple_of(m) {
                return Err(invalid(
                    "Training request ENCODER_PARAMETER_PQ_M is not divisible by vector dimensions",
                ));
            }
            Ok(Some(Pq { m, code_size }))
        }
        // ALLOC: error path; one message.
        other => Err(action_invalid(&format!("Invalid encoder name: {other}"))),
    }
}

/// Validates a training body.
///
/// # Errors
///
/// 400 with the plugin's exception type and message for the first rule the
/// body breaks.
pub(crate) fn parse(body: &Value) -> Result<TrainRequest, RestError> {
    let Some(body) = body.as_object() else {
        return Err(illegal("the request body must be an object"));
    };
    let training_index = text(body, "training_index")
        .ok_or_else(|| illegal("Request did not set \"training_index."))?;
    let training_field = text(body, "training_field")
        .ok_or_else(|| illegal("Request did not set \"training_field."))?;
    let dimension = body
        .get("dimension")
        .and_then(Value::as_u64)
        .and_then(|d| u32::try_from(d).ok())
        .filter(|d| *d > 0)
        .ok_or_else(|| illegal("Request did not set \"dimension."))?;
    let method = body
        .get("method")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            illegal("At least \"[method]\", \"[mode]\" or \"[compression_level]\" needs to be set")
        })?;
    if text(method, "engine").is_some_and(|e| e.eq_ignore_ascii_case("lucene")) {
        return Err(invalid(
            "Cannot use \"lucene\" engine from training context",
        ));
    }
    let space = text(method, "space_type")
        .or_else(|| text(body, "space_type"))
        .unwrap_or("l2")
        .to_ascii_lowercase();
    let empty = Map::new();
    let params = method
        .get("parameters")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut pq = None;
    for (key, value) in params {
        match key.as_str() {
            "nlist" | "nprobes" => {}
            "encoder" => {
                pq = pq_of(value, dimension)?;
            }
            other
                if text(method, "name") == Some("hnsw")
                    && matches!(other, "m" | "ef_construction") => {}
            other => {
                // ALLOC: error path; one message.
                return Err(action_invalid(&format!(
                    "Unknown parameter '{other}' found"
                )));
            }
        }
    }
    let name = text(method, "name").unwrap_or("");
    match name {
        "ivf" => {}
        // Observed only: the stock plugin refuses an `hnsw` method with a `pq`
        // encoder with this message even where the dimension divides.
        "hnsw" if pq.is_some() => {
            return Err(invalid(
                "Training request ENCODER_PARAMETER_PQ_M is not divisible by vector dimensions",
            ));
        }
        "hnsw" => return Err(action_invalid("Method does not require training.")),
        // ALLOC: error path; one message.
        other => return Err(action_invalid(&format!("Invalid method name: {other}"))),
    }
    Ok(TrainRequest {
        // ALLOC: one copy of each request string, bounded by the request body.
        training_index: training_index.to_owned(),
        training_field: training_field.to_owned(),
        dimension,
        space,
        nlist: positive(params, "nlist", 4)?,
        nprobes: positive(params, "nprobes", 1)?,
        pq,
        description: text(body, "description").unwrap_or("").to_owned(),
        max_training_vectors: body
            .get("max_training_vector_count")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_TRAINING_VECTORS),
        search_size: body
            .get("search_size")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(10_000),
    })
}

#[cfg(test)]
#[path = "knn_train_request_tests.rs"]
mod tests;
