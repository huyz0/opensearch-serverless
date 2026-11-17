//! A stored source as `XContentParser.map()` reads it (every object a
//! `java.util.HashMap`) and as Jackson writes the maps back, at OpenSearch
//! 3.10.0: what a filtered `_source` is made of.

use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::aggs::metric::double::write_java_double;
use crate::aggs::metric::hash_order::java_string_hash;
use crate::dsl::tokens::{Kind, Parser};

/// A source value as `XContentParser.map()` reads it.
#[derive(Debug, PartialEq)]
pub(super) enum Value<'a> {
    /// An object's entries in the order first seen, a repeated key keeping
    /// its first place and its last value, as `HashMap.put` does.
    Object(Vec<(&'a str, Value<'a>)>),
    Array(Vec<Value<'a>>),
    Str(&'a str),
    /// A number's text as written.
    Num(&'a str),
    Bool(bool),
    Null,
}

pub(super) fn read<'a>(p: &mut Parser<'a>, kind: Kind) -> Value<'a> {
    match kind {
        Kind::StartObject => {
            // ALLOC: one entry a key, bounded by the stored source.
            let mut entries: Vec<(&str, Value<'_>)> = Vec::new();
            let mut key = "";
            while let Some(k) = p.next_token() {
                match k {
                    Kind::EndObject => break,
                    Kind::FieldName => key = p.text(),
                    k => {
                        let v = read(p, k);
                        match entries.iter_mut().find(|(n, _)| *n == key) {
                            Some(entry) => entry.1 = v,
                            None => entries.push((key, v)),
                        }
                    }
                }
            }
            Value::Object(in_hash_order(entries))
        }
        Kind::StartArray => {
            // ALLOC: one entry a value, bounded by the stored source.
            let mut values = Vec::new();
            while let Some(k) = p.next_token() {
                if k == Kind::EndArray {
                    break;
                }
                values.push(read(p, k));
            }
            Value::Array(values)
        }
        Kind::String => Value::Str(p.text()),
        Kind::Number => Value::Num(p.text()),
        Kind::Bool => Value::Bool(p.text() == "true"),
        _ => Value::Null,
    }
}

/// An object's entries in the order its `HashMap` iterates them, which is
/// the order they are put into a filtered map: a key's place within a
/// bucket of the filtered map follows it.
pub(super) fn in_hash_order<'a>(entries: Vec<(&'a str, Value<'a>)>) -> Vec<(&'a str, Value<'a>)> {
    // ALLOC: one key an entry, at most the object's keys.
    let keys: Vec<&str> = entries.iter().map(|(k, _)| *k).collect();
    // ALLOC: as above.
    let mut order = Vec::with_capacity(keys.len());
    java_hash_order(&keys, &mut order);
    // ALLOC: as above.
    let mut slots: Vec<Option<(&str, Value<'_>)>> = entries.into_iter().map(Some).collect();
    order
        .iter()
        .filter_map(|&i| slots.get_mut(i).and_then(Option::take))
        // ALLOC: as above.
        .collect()
}

/// `HashMap.hash`: the hash's high half folded into its low.
pub(super) fn spread(s: &str) -> u32 {
    let h = java_string_hash(s).cast_unsigned();
    h ^ (h >> 16)
}

/// The order a `new HashMap<>()` iterates distinct keys put in the order
/// given: by bucket, and within a bucket in the order put. The table starts
/// at 16 and doubles when the map passes three quarters of it, or when a
/// ninth key would join a bucket of a table under 64 (as
/// `aggs::metric::hash_order` models, here without its quadratic count).
/// A bucket of more than eight keys in a larger table becomes a tree, which
/// is not modelled.
pub(super) fn java_hash_order(keys: &[&str], order: &mut Vec<usize>) {
    let mut cap: u32 = 16;
    // Keys per bucket while the table is under 64, where a full bucket
    // resizes it.
    let mut counts = [0u8; 64];
    let bucket =
        |k: &str, cap: u32| usize::try_from(spread(k) & cap.saturating_sub(1)).unwrap_or(0);
    for (size, key) in keys.iter().enumerate() {
        if cap < 64 {
            let b = bucket(key, cap);
            if counts.get(b).is_some_and(|&n| n >= 8) {
                cap = cap.saturating_mul(2);
                counts = [0; 64];
                for k in keys.get(..size).unwrap_or_default() {
                    if let Some(n) = counts.get_mut(bucket(k, cap)) {
                        *n = n.saturating_add(1);
                    }
                }
            }
        }
        // Past 63 buckets the counts are no longer read.
        if let Some(n) = counts.get_mut(bucket(key, cap)) {
            *n = n.saturating_add(1);
        }
        if size.saturating_add(1)
            > usize::try_from((cap >> 2).saturating_mul(3)).unwrap_or(usize::MAX)
        {
            cap = cap.saturating_mul(2);
            counts = [0; 64];
            for k in keys.get(..=size).unwrap_or_default() {
                if let Some(n) = counts.get_mut(bucket(k, cap)) {
                    *n = n.saturating_add(1);
                }
            }
        }
    }
    order.clear();
    order.extend(0..keys.len());
    order.sort_by_key(|&i| keys.get(i).map_or(0, |k| bucket(k, cap)));
}

/// A number as `XContentParser.map()` reads it and Jackson writes it back:
/// an integer as its digits, anything else as a `Double`.
pub(super) fn write_number(out: &mut dyn Write, text: &str) -> io::Result<()> {
    if text.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) {
        return write_java_double(out, text.parse::<f64>().unwrap_or(f64::NAN));
    }
    // An integer: `-0` reads as `0`; any other text is written as it is.
    if text.trim_start_matches('-').bytes().all(|b| b == b'0') {
        return out.write_all(b"0");
    }
    out.write_all(text.as_bytes())
}

pub(super) fn write_value(
    out: &mut dyn Write,
    v: &Value<'_>,
    order: &mut Vec<usize>,
) -> io::Result<()> {
    match v {
        Value::Object(entries) => {
            // ALLOC: one key a entry, at most the object's keys.
            let keys: Vec<&str> = entries.iter().map(|(k, _)| *k).collect();
            java_hash_order(&keys, order);
            // ALLOC: as above.
            let ordered = order.clone();
            out.write_all(b"{")?;
            for (n, &i) in ordered.iter().enumerate() {
                let Some((key, value)) = entries.get(i) else {
                    continue;
                };
                if n > 0 {
                    out.write_all(b",")?;
                }
                write_json_str(out, key)?;
                out.write_all(b":")?;
                write_value(out, value, order)?;
            }
            out.write_all(b"}")
        }
        Value::Array(values) => {
            out.write_all(b"[")?;
            for (n, value) in values.iter().enumerate() {
                if n > 0 {
                    out.write_all(b",")?;
                }
                write_value(out, value, order)?;
            }
            out.write_all(b"]")
        }
        Value::Str(s) => write_json_str(out, s),
        Value::Num(text) => write_number(out, text),
        Value::Bool(true) => out.write_all(b"true"),
        Value::Bool(false) => out.write_all(b"false"),
        Value::Null => out.write_all(b"null"),
    }
}
