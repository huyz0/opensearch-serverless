//! A field mapper's parameters: their values, how each is read from a
//! mapping, written back and merged.
//!
//! Ported from `ParametrizedFieldMapper.Parameter` (server, OpenSearch
//! 3.10): a parameter has a default, a parser, a merge rule (updateable, or
//! equal) and a check deciding whether it is written. A field built by the
//! Java is `init`ed from its mapper, so every parameter holds a value and
//! "configured" means "differs from the default"; the same holds here.

use std::fmt::Write as _;

use super::error::MappingError;
use super::json::{Json, is_integral, java_cmp, java_float, java_number};
use super::numbers;

/// One parameter's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No value (a `null` default).
    Null,
    /// A flag.
    Bool(bool),
    /// A flag that remembers whether the mapping set it (`Explicit<Boolean>`).
    Explicit(bool, bool),
    /// An `int`.
    Int(i32),
    /// A `float` (`boost`).
    Float(f32),
    /// A string, an analyzer's name or a restricted choice.
    Str(String),
    /// `meta`: string keys to string values.
    Meta(Vec<(String, String)>),
    /// A numeric field's `null_value`, already of the field's type.
    Number(numbers::Num),
    /// A `knn_vector` field's `method`.
    Method(super::knn::KnnMethod),
}

impl Value {
    /// The value as `Objects.toString` shows it in a conflict message.
    pub fn conflict_text(&self) -> String {
        match self {
            Value::Null => "null".into(),
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            Value::Bool(b) | Value::Explicit(b, _) => b.to_string(),
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            Value::Int(i) => i.to_string(),
            Value::Float(f) => java_float(*f),
            Value::Str(s) => s.clone(),
            Value::Meta(m) => {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                let inner: Vec<String> = m.iter().map(|(k, v)| format!("{k}={v}")).collect();
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                format!("{{{}}}", inner.join(", "))
            }
            Value::Number(n) => n.java_string(),
            Value::Method(m) => m.name.clone(),
        }
    }

    /// Writes the value as its parameter's serializer does.
    pub fn write_json(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) | Value::Explicit(b, _) => {
                out.push_str(if *b { "true" } else { "false" })
            }
            Value::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Value::Float(f) => out.push_str(&java_float(*f)),
            Value::Str(s) => super::json::write_str(out, s),
            Value::Meta(m) => {
                out.push('{');
                for (n, (k, v)) in m.iter().enumerate() {
                    if n > 0 {
                        out.push(',');
                    }
                    super::json::write_str(out, k);
                    out.push(':');
                    super::json::write_str(out, v);
                }
                out.push('}');
            }
            Value::Number(n) => out.push_str(&n.java_string()),
            Value::Method(m) => m.write_json(out),
        }
    }

    /// The flag, for a [`Value::Bool`] or [`Value::Explicit`].
    pub fn flag(&self) -> bool {
        matches!(self, Value::Bool(true) | Value::Explicit(true, _))
    }

    /// The text, for a [`Value::Str`].
    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// How a parameter is read and written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `boolParam`: `true` or `false`, or their text.
    Bool,
    /// `explicitBoolParam`.
    Explicit,
    /// `intParam`.
    Int,
    /// `boostParam`, a float.
    Boost,
    /// `stringParam`.
    Str,
    /// `restrictedStringParam`: one of the listed values.
    Choice(&'static [&'static str]),
    /// `metaParam`.
    Meta,
    /// `analyzerParam`: an analyzer's name.
    Analyzer,
    /// A keyword or date `null_value`: a string, or null.
    NullString,
    /// A boolean `null_value`: a flag, or null.
    NullBool,
    /// A numeric `null_value`: a number of the field's type, or null.
    NullNumber,
    /// A `knn_vector` `dimension`.
    KnnDimension,
    /// A `knn_vector` `data_type`.
    KnnDataType,
    /// A `knn_vector` `method`.
    KnnMethod,
    /// A `knn_vector` top-level `space_type`.
    KnnSpace,
}

/// How a parameter is merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Any new value is taken.
    Updateable,
    /// Only the same value is taken.
    Fixed,
    /// `norms`: may go from true to false, never back.
    Norms,
}

/// One parameter of a field type.
#[derive(Clone, Copy, Debug)]
pub struct Def {
    /// Its name in a mapping.
    pub name: &'static str,
    /// How it is read and written.
    pub kind: Kind,
    /// How it is merged.
    pub merge: Merge,
    /// Its default.
    pub default: Default,
    /// Whether this writer indexes only the default: a field that sets it
    /// to anything else is refused as unsupported.
    pub only_default: bool,
}

/// A parameter's default.
#[derive(Clone, Copy, Debug)]
pub enum Default {
    /// `null`.
    Null,
    /// A flag.
    Bool(bool),
    /// An integer.
    Int(i32),
    /// A float.
    Float(f32),
    /// A string.
    Str(&'static str),
    /// No entries (`meta`).
    Empty,
    /// The value of another parameter of the same field, by its index
    /// (`search_analyzer` defaults to `analyzer`).
    Same(usize),
}

/// Reads a flag as `Booleans.parseBoolean(node.toString())` reads one.
pub fn parse_bool(node: &Json) -> Result<bool, MappingError> {
    let text = node.java_string();
    match text.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        _ => Err(MappingError::illegal_argument(&format!(
            "Failed to parse value [{text}] as only [true] or [false] are allowed."
        ))),
    }
}

/// Reads a flag as `XContentMapValues.nodeBooleanValue(node, name)` does:
/// `Could not convert [<name>] to boolean`, caused by the parse error.
pub fn named_bool(name: &str, node: &Json) -> Result<bool, MappingError> {
    parse_bool(node).map_err(|e| {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        MappingError::illegal_argument(&format!("Could not convert [{name}] to boolean"))
            .caused_by(e)
    })
}

/// Reads an `int` as `XContentMapValues.nodeIntegerValue` does: a number
/// through `Numbers.toIntExact`, anything else through `Integer.parseInt`.
pub fn parse_int(node: &Json) -> Result<i32, MappingError> {
    match node {
        Json::Num(text) if is_integral(text) => match text.parse::<i64>() {
            Ok(v) => i32::try_from(v).map_err(|_| MappingError::arithmetic("integer overflow")),
            Err(_) => Err(MappingError::arithmetic("BigInteger out of long range")),
        },
        Json::Num(text) => {
            let v: f64 = text.parse().unwrap_or(f64::NAN);
            if v.fract() != 0.0 || !v.is_finite() {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "{} is not an integer value",
                    java_number(text)
                )));
            }
            if v >= f64::from(i32::MIN) && v <= f64::from(i32::MAX) {
                // In range and integral, so the cast is exact.
                #[allow(clippy::cast_possible_truncation)]
                Ok(v as i32)
            } else {
                Err(MappingError::arithmetic("integer overflow"))
            }
        }
        other => {
            let text = other.java_string();
            java_parse_int(&text).ok_or_else(|| MappingError::number_format(&text))
        }
    }
}

/// `Integer.parseInt`: an optional sign and decimal digits, which is what
/// `i32`'s parse takes.
fn java_parse_int(text: &str) -> Option<i32> {
    text.parse().ok()
}

/// Reads a float as `XContentMapValues.nodeFloatValue` does.
pub fn parse_float(node: &Json) -> Result<f32, MappingError> {
    let text = match node {
        Json::Num(t) => t.clone(),
        other => other.java_string(),
    };
    numbers::java_parse_double(&text)
        .map(|v| {
            #[allow(clippy::cast_possible_truncation)]
            let f = v as f32;
            f
        })
        .ok_or_else(|| MappingError::number_format(&text))
}

/// Most entries a `meta` object may hold (`TypeParsers.META_MAX_ENTRIES`).
pub const META_MAX_ENTRIES: usize = 100;
/// Longest `meta` key, in code points (`TypeParsers.META_MAX_KEY_LENGTH`).
pub const META_MAX_KEY_LENGTH: usize = 255;
/// Longest `meta` value, in code points (`TypeParsers.META_MAX_VALUE_LENGTH`).
pub const META_MAX_VALUE_LENGTH: usize = 10_000;

/// Reads `meta` as `TypeParsers.parseMeta` does.
pub fn parse_meta(field: &str, node: &Json) -> Result<Vec<(String, String)>, MappingError> {
    let Json::Obj(members) = node else {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::mapper_parsing(&format!(
            "[meta] must be an object, got {}[{}] for field [{field}]",
            java_class(node),
            node.java_string()
        )));
    };
    if members.len() > META_MAX_ENTRIES {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::mapper_parsing(&format!(
            "[meta] can't have more than {META_MAX_ENTRIES} entries, but got {} on field [{field}]",
            members.len()
        )));
    }
    // ALLOC: at most META_MAX_ENTRIES entries.
    let mut out = Vec::with_capacity(members.len());
    for (key, value) in members {
        if key.chars().count() > META_MAX_KEY_LENGTH {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::mapper_parsing(&format!(
                "[meta] keys can't be longer than {META_MAX_KEY_LENGTH} chars, but got [{key}] for field [{field}]"
            )));
        }
        match value {
            Json::Null => {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "[meta] values can't be null (field [{field}])"
                )));
            }
            Json::Str(s) if s.chars().count() > META_MAX_VALUE_LENGTH => {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "[meta] values can't be longer than {META_MAX_VALUE_LENGTH} chars, but got [{s}] for field [{field}]"
                )));
            }
            Json::Str(s) => out.push((key.clone(), s.clone())),
            other => {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "[meta] values can only be strings, but got {}[{}] for field [{field}]",
                    java_class(other),
                    other.java_string()
                )));
            }
        }
    }
    out.sort_by(|a, b| java_cmp(&a.0, &b.0));
    Ok(out)
}

/// The package of [`java_class`]'s class.
pub fn java_package(node: &Json) -> &'static str {
    match java_class(node) {
        "BigInteger" => "java.math",
        "ArrayList" | "LinkedHashMap" => "java.util",
        _ => "java.lang",
    }
}

/// The simple name of the class Jackson reads a value as.
pub fn java_class(node: &Json) -> &'static str {
    match node {
        Json::Null => "null",
        Json::Bool(_) => "Boolean",
        Json::Num(t) if is_integral(t) => match t.parse::<i64>() {
            Ok(v) if i32::try_from(v).is_ok() => "Integer",
            Ok(_) => "Long",
            Err(_) => "BigInteger",
        },
        Json::Num(_) => "Double",
        Json::Str(_) => "String",
        Json::Arr(_) => "ArrayList",
        Json::Obj(_) => "LinkedHashMap",
    }
}
