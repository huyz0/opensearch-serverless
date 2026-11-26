//! A `QueryContainer` as the REST query object, for the covered query types.

use prost_reflect::{DynamicMessage, Value};
use serde_json::{Map, Number, Value as Json};

use super::fields::Fields;
use super::status::{INVALID_ARGUMENT, Status};

/// The query types a `QueryContainer` is read for; any other set one is refused.
pub const COVERED_QUERIES: [&str; 10] = [
    "match_all",
    "term",
    "terms",
    "match",
    "match_phrase",
    "range",
    "exists",
    "ids",
    "prefix",
    "bool",
];

/// One `QueryContainer` as the REST query object.
pub(crate) fn query(msg: &DynamicMessage, path: &str) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let mut chosen = None;
    for name in COVERED_QUERIES {
        let Some(q) = f.message(name) else { continue };
        let at = f.at(name);
        let body = match name {
            "term" => keyed(q, &at, |f| value(f, "value"))?,
            "terms" => terms(q, &at)?,
            "match" => keyed(q, &at, |f| value(f, "query"))?,
            "match_phrase" => keyed(q, &at, |f| {
                f.string("query")
                    .map(Json::from)
                    .ok_or_else(|| missing(&f.at("query")))
            })?,
            "range" => range(q, &at)?,
            "exists" => keyed_field(q, &at)?,
            "ids" => leaf(q, &at, |f| {
                let mut o = Map::new();
                // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
                o.insert("values".to_owned(), strings(f.repeated("values")));
                Ok(Json::Object(o))
            })?,
            "prefix" => keyed(q, &at, |f| {
                f.string("value")
                    .map(Json::from)
                    .ok_or_else(|| missing(&f.at("value")))
            })?,
            "bool" => boolean(q, &at)?,
            _ => leaf(q, &at, |_| Ok(Json::Object(Map::new())))?,
        };
        let mut o = Map::new();
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        o.insert(name.to_owned(), body);
        chosen = Some(Json::Object(o));
    }
    f.finish()?;
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    chosen.ok_or_else(|| Status::new(INVALID_ARGUMENT, format!("{path} names no query")))
}

fn missing(path: &str) -> Status {
    // ALLOC: an error text naming a field path, a few bytes.
    Status::new(INVALID_ARGUMENT, format!("{path} is required"))
}

fn strings(items: &[Value]) -> Json {
    Json::Array(
        items
            .iter()
            .filter_map(Value::as_str)
            .map(Json::from)
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            .collect(),
    )
}

/// A query message with no `field`: `body` reads the fields it names.
fn leaf(
    msg: &DynamicMessage,
    path: &str,
    body: impl FnOnce(&mut Fields<'_>) -> Result<Json, Status>,
) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let out = body(&mut f)?;
    f.finish()?;
    Ok(out)
}

/// A query message of the shape `{field, <value>}` as `{field: value}`.
fn keyed(
    msg: &DynamicMessage,
    path: &str,
    body: impl FnOnce(&mut Fields<'_>) -> Result<Json, Status>,
) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let field = f.string("field").ok_or_else(|| missing(&f.at("field")))?;
    let value = body(&mut f)?;
    f.finish()?;
    let mut o = Map::new();
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    o.insert(field.to_owned(), value);
    Ok(Json::Object(o))
}

fn keyed_field(msg: &DynamicMessage, path: &str) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let field = f.string("field").ok_or_else(|| missing(&f.at("field")))?;
    f.finish()?;
    let mut o = Map::new();
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    o.insert("field".to_owned(), Json::from(field));
    Ok(Json::Object(o))
}

/// A `FieldValue` field as JSON.
fn value(f: &mut Fields<'_>, name: &str) -> Result<Json, Status> {
    let at = f.at(name);
    let msg = f.message(name).ok_or_else(|| missing(&at))?;
    field_value(msg, &at)
}

fn field_value(msg: &DynamicMessage, path: &str) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let mut out = None;
    if let Some(b) = f.boolean("bool") {
        out = Some(Json::Bool(b));
    }
    if let Some(s) = f.string("string") {
        out = Some(Json::from(s));
    }
    if f.enum_word("null_value").is_some() {
        out = Some(Json::Null);
    }
    if let Some(n) = f.message("general_number") {
        let mut g = Fields::new(n, &f.at("general_number"));
        for name in ["int32_value", "int64_value"] {
            if let Some(n) = g.int(name) {
                out = Some(Json::from(n));
            }
        }
        if let Some(n) = g.uint("uint64_value") {
            out = Some(Json::from(n));
        }
        for name in ["float_value", "double_value"] {
            if let Some(n) = g.float(name) {
                out = Some(number(n)?);
            }
        }
        g.finish()?;
    }
    f.finish()?;
    out.ok_or_else(|| missing(path))
}

/// A double as JSON: a whole number is written as an integer, as the REST client would.
fn number(n: f64) -> Result<Json, Status> {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9e15 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "checked whole and below 2^53 above"
        )]
        return Ok(Json::from(n as i64));
    }
    Number::from_f64(n)
        .map(Json::Number)
        .ok_or_else(|| Status::new(INVALID_ARGUMENT, "a number is not finite"))
}

/// `terms`: one field and its list of values; a lookup is refused.
fn terms(msg: &DynamicMessage, path: &str) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let entries = f.entries("terms");
    let [(field, entry)] = entries.as_slice() else {
        return Err(Status::new(
            INVALID_ARGUMENT,
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            format!("{path}.terms must name exactly one field"),
        ));
    };
    let Value::Message(entry) = entry else {
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        return Err(missing(&format!("{path}.terms[{field}]")));
    };
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    let at = format!("{path}.terms[{field}]");
    let mut e = Fields::new(entry, &at);
    let Some(list) = e.message("value") else {
        // A lookup, or nothing: the lookup is refused by path, and nothing is a malformed entry.
        e.finish()?;
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        return Err(missing(&format!("{at}.value")));
    };
    let mut l = Fields::new(list, &e.at("value"));
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    let mut values = Vec::new();
    for (i, v) in l.repeated("field_value_array").iter().enumerate() {
        let Value::Message(v) = v else { continue };
        values.push(field_value(
            v,
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            &format!("{}[{i}]", l.at("field_value_array")),
        )?);
    }
    l.finish()?;
    e.finish()?;
    f.finish()?;
    let mut o = Map::new();
    o.insert(field.clone(), Json::Array(values));
    Ok(Json::Object(o))
}

/// `range`: a number or a date range on one field, with the four bounds.
fn range(msg: &DynamicMessage, path: &str) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let mut out = None;
    for (name, numeric) in [("number_range_query", true), ("date_range_query", false)] {
        let Some(r) = f.message(name) else { continue };
        let at = f.at(name);
        let mut g = Fields::new(r, &at);
        let field = g.string("field").ok_or_else(|| missing(&g.at("field")))?;
        let mut bounds = Map::new();
        for bound in ["gt", "gte", "lt", "lte"] {
            if numeric {
                if let Some(n) = g.float(bound) {
                    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
                    bounds.insert(bound.to_owned(), number(n)?);
                }
            } else if let Some(s) = g.string(bound) {
                // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
                bounds.insert(bound.to_owned(), Json::from(s));
            }
        }
        g.finish()?;
        let mut o = Map::new();
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        o.insert(field.to_owned(), Json::Object(bounds));
        out = Some(Json::Object(o));
    }
    f.finish()?;
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    out.ok_or_else(|| Status::new(INVALID_ARGUMENT, format!("{path} names no range")))
}

/// `bool`: the four clause lists and `minimum_should_match`.
fn boolean(msg: &DynamicMessage, path: &str) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let mut o = Map::new();
    for clause in ["must", "filter", "should", "must_not"] {
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        let mut list = Vec::new();
        for (i, q) in f.repeated(clause).iter().enumerate() {
            let Value::Message(q) = q else { continue };
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            list.push(query(q, &format!("{}[{i}]", f.at(clause)))?);
        }
        if !list.is_empty() {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            o.insert(clause.to_owned(), Json::Array(list));
        }
    }
    if let Some(m) = f.message("minimum_should_match") {
        let mut g = Fields::new(m, &f.at("minimum_should_match"));
        if let Some(n) = g.int("int32") {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            o.insert("minimum_should_match".to_owned(), Json::from(n));
        }
        if let Some(s) = g.string("string") {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            o.insert("minimum_should_match".to_owned(), Json::from(s));
        }
        g.finish()?;
    }
    f.finish()?;
    Ok(Json::Object(o))
}
