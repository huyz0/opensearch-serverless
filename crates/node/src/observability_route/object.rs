//! What an observability object body holds: one object of one of the
//! plugin's types, kept as the plugin keeps it (only the fields it indexes).

use oss_rest::RestError;
use serde_json::{Map, Value};

use crate::route_util::refuse;

/// The object types the plugin stores, as the key that holds the body, with
/// the fields it keeps of each; the first type present in a body is the
/// body's type.
pub(super) const TYPES: &[(&str, &[&str])] = &[
    (
        "savedQuery",
        &[
            "name",
            "description",
            "query",
            "selected_date_range",
            "selected_timestamp",
            "selected_fields",
        ],
    ),
    (
        "savedVisualization",
        &[
            "name",
            "description",
            "query",
            "type",
            "selected_date_range",
            "selected_timestamp",
            "selected_fields",
            "sub_type",
        ],
    ),
    (
        "application",
        &[
            "name",
            "description",
            "baseQuery",
            "servicesEntities",
            "traceGroups",
            "panelId",
        ],
    ),
    ("operationalPanel", &["name"]),
    (
        "notebook",
        &["name", "dateCreated", "dateModified", "paragraphs"],
    ),
];

/// The longest name an object may carry.
const MAX_NAME_BYTES: usize = 256;

/// A body that holds an object.
pub(super) struct Parsed {
    /// The type key.
    pub ty: &'static str,
    /// The object, reduced to the fields kept for its type.
    pub object: Map<String, Value>,
    /// The `objectId` the body names, if it does.
    pub object_id: Option<String>,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

/// The fields of `body` the plugin keeps, for the first known type in it.
///
/// # Errors
///
/// 400 for no body, a body that is not JSON, no known type, a type whose
/// value is not an object, or a name that is not a string of at most 256
/// bytes.
pub(super) fn parse(raw: &[u8]) -> Result<Parsed, RestError> {
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Err(refuse(400, "parse_exception", "request body is required"));
    }
    let body: Value = serde_json::from_slice(raw)
        .map_err(|e| refuse(400, "parse_exception", format!("invalid body: {e}")))?;
    let Some(map) = body.as_object() else {
        return Err(bad("Object type field absent"));
    };
    let Some((ty, kept)) = TYPES.iter().find(|(t, _)| map.contains_key(*t)) else {
        return Err(bad("Object type field absent"));
    };
    let Some(fields) = map.get(*ty).and_then(Value::as_object) else {
        return Err(refuse(
            400,
            "parsing_exception",
            format!(
                "Failed to parse object: expecting token of type [START_OBJECT] but found [{}]",
                token(&map[*ty])
            ),
        ));
    };
    match fields.get("name") {
        Some(Value::String(n)) if n.len() > MAX_NAME_BYTES => {
            return Err(bad(format!("[{ty}.name] is over {MAX_NAME_BYTES} bytes")));
        }
        Some(Value::String(_)) | None => {}
        Some(_) => return Err(bad(format!("[{ty}.name] must be a string"))),
    }
    // ALLOC: the kept fields of one object, bounded by the body's size.
    let object: Map<String, Value> = fields
        .iter()
        .filter(|(k, _)| kept.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let object_id = map
        .get("objectId")
        .and_then(Value::as_str)
        // ALLOC: one id.
        .map(str::to_owned);
    Ok(Parsed {
        ty,
        object,
        object_id,
    })
}

/// Whether `key` names a stored object's own bookkeeping, not its type.
pub(super) fn is_meta(key: &str) -> bool {
    matches!(key, "createdTimeMs" | "lastUpdatedTimeMs" | "version")
}

/// The type key and body of a stored document.
pub(super) fn type_of(doc: &Value) -> Option<(&str, &Map<String, Value>)> {
    doc.as_object()?
        .iter()
        .find(|(k, _)| !is_meta(k))
        .and_then(|(k, v)| Some((k.as_str(), v.as_object()?)))
}

/// The name of the JSON token a value starts with, as the parser names it.
fn token(v: &Value) -> &'static str {
    match v {
        Value::Null => "VALUE_NULL",
        Value::Bool(true) => "VALUE_TRUE",
        Value::Bool(false) => "VALUE_FALSE",
        Value::Number(n) if n.is_f64() => "VALUE_NUMBER_FLOAT",
        Value::Number(_) => "VALUE_NUMBER_INT",
        Value::String(_) => "VALUE_STRING",
        Value::Array(_) => "START_ARRAY",
        Value::Object(_) => "START_OBJECT",
    }
}
