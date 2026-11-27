//! `ext.ltr_log` of the learning to rank plugin: which feature vectors a
//! search logs into each hit's `fields._ltrlog`. Every other `ext` section
//! is refused, as no other search extension is served.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, int_value};

/// The most log specs one search asks for.
pub const MAX_LOGS: usize = 16;

/// One `log_specs` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct LogSpec {
    /// The name the log is written under.
    pub name: String,
    /// The `_name` of the `sltr` query whose feature set is logged.
    pub named_query: Option<String>,
    /// The rescore whose `sltr` is logged, by position.
    pub rescore_index: Option<usize>,
    /// Whether a feature that did not match logs 0 rather than no value.
    pub missing_as_zero: bool,
}

fn bad(p: &Parser<'_>, reason: &str) -> DslError {
    // ALLOC: one string or message, bounded by the body's recording.
    DslError::parsing(p.location(), format!("[ltr_log] {reason}"))
}

fn spec(p: &mut Parser<'_>) -> Result<LogSpec, DslError> {
    let (mut name, mut named, mut index, mut zero) = (None, None, None, false);
    while let Some(kind) = p.next_token() {
        match kind {
            Kind::EndObject => break,
            Kind::FieldName => {
                // ALLOC: one string or message, bounded by the body's recording.
                let key = p.text().to_owned();
                p.next_token();
                match key.as_str() {
                    // ALLOC: one name, bounded by the body's recording.
                    "name" => name = Some(p.text().to_owned()),
                    // ALLOC: one name, bounded by the body's recording.
                    "named_query" => named = Some(p.text().to_owned()),
                    "rescore_index" => {
                        let n = int_value(p)?;
                        index = Some(
                            usize::try_from(n)
                                .map_err(|_| bad(p, "[rescore_index] must not be negative"))?,
                        );
                    }
                    "missing_as_zero" => zero = bool_value(p)?,
                    // ALLOC: error path; one message.
                    other => return Err(bad(p, &format!("unknown field [{other}]"))),
                }
            }
            _ => return Err(bad(p, "malformed")),
        }
    }
    let Some(name) = name else {
        return Err(bad(p, "a log spec needs a [name]"));
    };
    if named.is_some() == index.is_some() {
        return Err(bad(
            p,
            "a log spec needs one of [named_query] or [rescore_index]",
        ));
    }
    Ok(LogSpec {
        name,
        named_query: named,
        rescore_index: index,
        missing_as_zero: zero,
    })
}

fn specs(p: &mut Parser<'_>) -> Result<Vec<LogSpec>, DslError> {
    // ALLOC: one spec an entry, at most MAX_LOGS.
    let mut out = Vec::new();
    match p.next_token() {
        Some(Kind::StartObject) => out.push(spec(p)?),
        Some(Kind::StartArray) => loop {
            match p.next_token() {
                Some(Kind::StartObject) => out.push(spec(p)?),
                Some(Kind::EndArray) | None => break,
                Some(_) => return Err(bad(p, "[log_specs] holds objects")),
            }
        },
        _ => return Err(bad(p, "[log_specs] must be an object or an array")),
    }
    if out.len() > MAX_LOGS {
        return Err(bad(p, "too many log specs"));
    }
    Ok(out)
}

/// The `ext` object, the parser on its start: the log specs of its
/// `ltr_log`.
///
/// # Errors
///
/// A [`DslError`] for a malformed spec and for any other section.
pub fn parse_ext(p: &mut Parser<'_>) -> Result<Vec<LogSpec>, DslError> {
    // ALLOC: one string or message, bounded by the body's recording.
    let mut out = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(out),
            Some(Kind::FieldName) if p.text() == "ltr_log" => {
                if p.next_token() != Some(Kind::StartObject) {
                    return Err(bad(p, "must be an object"));
                }
                loop {
                    match p.next_token() {
                        Some(Kind::EndObject) | None => break,
                        Some(Kind::FieldName) if p.text() == "log_specs" => {
                            out.extend(specs(p)?);
                        }
                        Some(_) => return Err(bad(p, "takes only [log_specs]")),
                    }
                }
            }
            Some(Kind::FieldName) => {
                // ALLOC: one string or message, bounded by the body's recording.
                return Err(DslError::unsupported(&format!(
                    "the search request's [ext] section [{}]",
                    p.text()
                )));
            }
            Some(_) => return Err(bad(p, "malformed [ext]")),
        }
    }
}
