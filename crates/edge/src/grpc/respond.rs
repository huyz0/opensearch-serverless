//! The node's JSON answer mapped onto a response message, field by field.
//!
//! A key the response message has no field for fails the call with `INTERNAL` naming it, so a
//! gap in the mapping shows as a failure and not as a silent drop. The one exception is
//! `ErrorCause`, whose extra keys are carried in its `metadata` map, as upstream does.

use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor, Value};
use serde_json::Value as Json;

use super::status::{INTERNAL, Status};

fn gap(path: &str, what: &str) -> Status {
    // ALLOC: an error text naming a key path, a few bytes.
    Status::new(
        INTERNAL,
        format!("the answer has {what} at {path}, which the gRPC mapping does not cover"),
    )
}

/// The JSON name of a field: a leading `x_` stands for `_`, as in `x_id` and `_id`.
fn json_name(field: &FieldDescriptor) -> String {
    match field.name().strip_prefix("x_") {
        // ALLOC: a field name, a few bytes.
        Some(rest) => format!("_{rest}"),
        None => field.name().to_owned(),
    }
}

/// Builds `desc` from a JSON object.
pub(crate) fn message(
    desc: &MessageDescriptor,
    json: &Json,
    path: &str,
) -> Result<DynamicMessage, Status> {
    let Json::Object(object) = json else {
        return Err(gap(path, "a value that is not an object"));
    };
    let mut out = DynamicMessage::new(desc.clone());
    let mut extra = serde_json::Map::new();
    for (key, value) in object {
        let Some(field) = desc.fields().find(|f| json_name(f) == *key) else {
            if desc.name() == "ErrorCause" {
                extra.insert(key.clone(), value.clone());
                continue;
            }
            // ALLOC: a key path, a few bytes.
            return Err(gap(&format!("{path}.{key}"), "a key"));
        };
        if value.is_null() {
            continue;
        }
        // ALLOC: a key path, a few bytes.
        let at = format!("{path}.{key}");
        let converted = field_value(&field, value, &at)?;
        out.set_field(&field, converted);
    }
    if !extra.is_empty()
        && let Some(field) = desc.get_field_by_name("metadata")
    {
        out.set_field(&field, Value::Message(object_map(&Json::Object(extra))?));
    }
    Ok(out)
}

fn field_value(field: &FieldDescriptor, json: &Json, path: &str) -> Result<Value, Status> {
    if field.is_list() {
        let Json::Array(items) = json else {
            return Err(gap(path, "a value that is not an array"));
        };
        // ALLOC: the answer being mapped, within `grpc_max_message` and charged at start.
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            // ALLOC: an element path, a few bytes.
            out.push(scalar(&field.kind(), item, &format!("{path}[{i}]"))?);
        }
        return Ok(Value::List(out));
    }
    if field.is_map() {
        return Err(gap(path, "a map"));
    }
    scalar(&field.kind(), json, path)
}

fn scalar(kind: &Kind, json: &Json, path: &str) -> Result<Value, Status> {
    let wrong = || gap(path, "a value of another type");
    Ok(match kind {
        Kind::Bool => Value::Bool(json.as_bool().ok_or_else(wrong)?),
        Kind::Int32 => Value::I32(
            json.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(wrong)?,
        ),
        Kind::Int64 => Value::I64(json.as_i64().ok_or_else(wrong)?),
        Kind::Float => Value::F32(json.as_f64().ok_or_else(wrong)? as f32),
        Kind::Double => Value::F64(json.as_f64().ok_or_else(wrong)?),
        // ALLOC: the answer being mapped, within `grpc_max_message` and charged at start.
        Kind::String => Value::String(json.as_str().ok_or_else(wrong)?.to_owned()),
        Kind::Message(desc) if desc.full_name() == "org.opensearch.protobufs.ObjectMap" => {
            Value::Message(object_map(json)?)
        }
        Kind::Message(desc) => Value::Message(message(desc, json, path)?),
        _ => return Err(gap(path, "a type the mapping does not read")),
    })
}

/// A JSON value as `ObjectMap`, the schema's free-form object.
pub(crate) fn object_map(json: &Json) -> Result<DynamicMessage, Status> {
    let map_desc = super::schema::message("ObjectMap");
    let Json::Object(object) = json else {
        return Err(gap("ObjectMap", "a value that is not an object"));
    };
    let mut out = DynamicMessage::new(map_desc.clone());
    // ALLOC: the answer being mapped, within `grpc_max_message` and charged at start.
    let mut entries = std::collections::HashMap::new();
    for (key, value) in object {
        entries.insert(
            MapKey::String(key.clone()),
            Value::Message(object_value(value)?),
        );
    }
    if let Some(field) = map_desc.get_field_by_name("fields") {
        out.set_field(&field, Value::Map(entries));
    }
    Ok(out)
}

fn object_value(json: &Json) -> Result<DynamicMessage, Status> {
    let desc = super::schema::message("ObjectMap.Value");
    let mut out = DynamicMessage::new(desc.clone());
    let (name, value) = match json {
        Json::Null => ("null_value", Value::EnumNumber(1)),
        Json::Bool(b) => ("bool", Value::Bool(*b)),
        Json::Number(n) => match n.as_i64() {
            Some(i) => ("int64", Value::I64(i)),
            None => ("double", Value::F64(n.as_f64().unwrap_or(0.0))),
        },
        Json::String(s) => ("string", Value::String(s.clone())),
        Json::Object(_) => ("object_map", Value::Message(object_map(json)?)),
        Json::Array(items) => {
            let list_desc = super::schema::message("ObjectMap.ListValue");
            let mut list = DynamicMessage::new(list_desc.clone());
            // ALLOC: the answer being mapped, within `grpc_max_message` and charged at start.
            let mut values = Vec::new();
            for item in items {
                values.push(Value::Message(object_value(item)?));
            }
            if let Some(field) = list_desc.get_field_by_name("value") {
                list.set_field(&field, Value::List(values));
            }
            ("list_value", Value::Message(list))
        }
    };
    if let Some(field) = desc.get_field_by_name(name) {
        out.set_field(&field, value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_reflect::ReflectMessage;
    use serde_json::json;

    fn built(name: &str, json: &Json) -> Result<DynamicMessage, Status> {
        message(&super::super::schema::message(name), json, name)
    }

    fn field(m: &DynamicMessage, name: &str) -> Value {
        m.get_field_by_name(name).expect("set").into_owned()
    }

    #[test]
    fn keys_map_to_fields_with_the_underscore_prefix() {
        let m = built(
            "BulkResponse",
            &json!({"took": 3, "errors": true, "ingest_took": 1, "items": [
                {"delete": {"_index": "a", "_id": "9", "status": 404, "_shards": {"total": 1, "successful": 1, "failed": 0}, "forced_refresh": true}}
            ]}),
        )
        .expect("maps");
        assert_eq!(field(&m, "took"), Value::I64(3));
        assert_eq!(field(&m, "errors"), Value::Bool(true));
        assert_eq!(field(&m, "ingest_took"), Value::I64(1));
        let Value::List(items) = field(&m, "items") else {
            panic!("a list")
        };
        assert_eq!(items.len(), 1);
        let Value::Message(item) = &items[0] else {
            panic!("a message")
        };
        let Value::Message(del) = field(item, "delete") else {
            panic!("a message")
        };
        assert_eq!(field(&del, "x_index"), Value::String("a".to_owned()));
        assert_eq!(field(&del, "status"), Value::I32(404));
        assert_eq!(field(&del, "forced_refresh"), Value::Bool(true));
    }

    #[test]
    fn nulls_are_skipped_and_gaps_are_internal() {
        let m = built(
            "BulkResponse",
            &json!({"took": 1, "errors": false, "ingest_took": null, "items": []}),
        )
        .expect("maps");
        assert!(!m.has_field_by_name("ingest_took"));
        for (name, json, what) in [
            (
                "BulkResponse",
                json!({"surprise": 1}),
                "BulkResponse.surprise",
            ),
            ("BulkResponse", json!([]), "BulkResponse"),
            ("BulkResponse", json!({"took": "x"}), "BulkResponse.took"),
            ("BulkResponse", json!({"took": 1.5}), "BulkResponse.took"),
            ("BulkResponse", json!({"items": {}}), "BulkResponse.items"),
            (
                "ResponseItem",
                json!({"status": 4_000_000_000i64}),
                "ResponseItem.status",
            ),
            ("ResponseItem", json!({"_id": 1}), "ResponseItem._id"),
            (
                "ResponseItem",
                json!({"forced_refresh": "yes"}),
                "ResponseItem.forced_refresh",
            ),
        ] {
            let err = built(name, &json).expect_err(what);
            assert_eq!(err.code, INTERNAL, "{what}");
            assert!(err.message.contains(what), "{what}: {}", err.message);
        }
    }

    #[test]
    fn an_error_cause_keeps_its_extra_keys_as_metadata() {
        let m = built(
            "ErrorCause",
            &json!({"type": "t", "reason": "r", "index": "a", "shard": 2, "caused_by": {"type": "c"},
                    "root_cause": [{"type": "x"}], "extra": {"k": [1, 2.5, "s", true, null, {"n": 1}]}}),
        )
        .expect("maps");
        assert_eq!(field(&m, "type"), Value::String("t".to_owned()));
        let Value::Message(meta) = field(&m, "metadata") else {
            panic!("a message")
        };
        let Value::Map(fields) = field(&meta, "fields") else {
            panic!("a map")
        };
        assert_eq!(fields.len(), 3, "index, shard and extra");
        assert!(m.has_field_by_name("caused_by"));
        let Value::List(roots) = field(&m, "root_cause") else {
            panic!("a list")
        };
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn numbers_and_free_form_fields_keep_their_type() {
        let m = built("Explanation", &json!({"description": "d", "value": 1.5})).expect("maps");
        assert_eq!(field(&m, "value"), Value::F32(1.5));
        let m = built("LatLonGeoLocation", &json!({"lat": 2.5, "lon": 3.0})).expect("maps");
        assert_eq!(field(&m, "lat"), Value::F64(2.5));
        let m = built(
            "ResponseItem",
            &json!({"get": {"found": true, "fields": {"a": 1}}}),
        )
        .expect("maps");
        let Value::Message(get) = field(&m, "get") else {
            panic!("a message")
        };
        let Value::Message(fields) = field(&get, "fields") else {
            panic!("a message")
        };
        assert_eq!(fields.descriptor().name(), "ObjectMap");
    }

    #[test]
    fn object_values_cover_every_json_type() {
        let map = object_map(&json!({"n": null, "b": true, "i": 7, "d": 0.5, "s": "x", "o": {"a": 1}, "l": [1, "two"]}))
            .expect("maps");
        let Value::Map(fields) = field(&map, "fields") else {
            panic!("a map")
        };
        let kind = |k: &str| {
            let Some(Value::Message(v)) = fields.get(&MapKey::String(k.to_owned())) else {
                panic!("{k}")
            };
            v.fields()
                .next()
                .expect("one field set")
                .0
                .name()
                .to_owned()
        };
        assert_eq!(kind("n"), "null_value");
        assert_eq!(kind("b"), "bool");
        assert_eq!(kind("i"), "int64");
        assert_eq!(kind("d"), "double");
        assert_eq!(kind("s"), "string");
        assert_eq!(kind("o"), "object_map");
        assert_eq!(kind("l"), "list_value");
        assert_eq!(object_map(&json!(1)).err().map(|s| s.code), Some(INTERNAL));
    }
}
