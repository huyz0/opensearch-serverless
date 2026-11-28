//! Shared fixtures: the mapping every corpus statement runs against.
#![allow(dead_code)]

use oss_query::{Limits, Mapping};
use serde_json::{Value, json};

/// The accounts mapping used by the corpus (provisional: hand-written).
pub fn mapping() -> Mapping {
    Mapping::from_json(&json!({"properties": {
        "firstname": {"type": "text", "fields": {"keyword": {"type": "keyword"}}},
        "lastname": {"type": "keyword"},
        "age": {"type": "long"},
        "balance": {"type": "double"},
        "active": {"type": "boolean"},
        "created": {"type": "date"},
        "city": {"type": "keyword"},
        "address": {"type": "text"},
        "ip": {"type": "ip"},
        "employer": {"type": "object", "properties": {"name": {"type": "keyword"}}}
    }}))
    .expect("the fixture mapping is valid")
}

/// The search body a statement lowers to, or the refusal.
pub fn lower(sql: &str) -> Result<Value, oss_query::Error> {
    let stmt = oss_query::parse(sql, &Limits::default())?;
    let oss_query::Statement::Select(sel) = stmt else {
        panic!("not a select: {sql}");
    };
    Ok(oss_query::plan(&sel, &mapping(), 200)?.body().clone())
}
