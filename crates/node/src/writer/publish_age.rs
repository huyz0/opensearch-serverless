//! How long an index lets a write wait to be published: its
//! `index.refresh_interval`, read from the descriptor's settings when a shard
//! of it is opened.
//!
//! Ported from `BackgroundReconciler.setLazyPublish` (a refresh interval
//! bounds the age of the oldest unpublished write; `-1` leaves it to the
//! operation count and the release).

use oss_writer::mapping::json::{Json, parse};
use oss_writer::publish::lazy::PublishAge;

use crate::time_value::time_millis;

const KEY: &str = "index.refresh_interval";

/// The member `key` of settings JSON, flat (`"index.a.b"`) or nested
/// (`{"index":{"a":{"b":..}}}`).
pub(super) fn find<'a>(settings: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    if let Some((_, v)) = settings.iter().find(|(k, _)| k == key) {
        return Some(v);
    }
    let mut members = settings;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        let (_, value) = members.iter().find(|(k, _)| k == part)?;
        if parts.peek().is_none() {
            return Some(value);
        }
        match value {
            Json::Obj(inner) => members = inner,
            _ => return None,
        }
    }
    None
}

/// The publish age an index with `settings` (descriptor settings JSON) asks
/// for: its `index.refresh_interval`, or the node's age when it sets none or
/// sets one that does not parse.
#[must_use]
pub fn publish_age_of(settings: &[u8]) -> PublishAge {
    let Ok(Json::Obj(members)) = parse(settings) else {
        return PublishAge::Node;
    };
    let value = match find(&members, KEY) {
        Some(Json::Str(text) | Json::Num(text)) => text.trim(),
        _ => return PublishAge::Node,
    };
    if value == "-1" {
        return PublishAge::Never;
    }
    time_millis(KEY, value).map_or(PublishAge::Node, PublishAge::Millis)
}
