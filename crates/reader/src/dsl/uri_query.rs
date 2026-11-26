//! The `q` URL parameter of `_search` and `_count`: a Lucene query string with its companions
//! `df`, `default_operator`, `analyzer`, `analyze_wildcard` and `lenient`.

use super::{DslError, Query, parse_chunks};

/// The URL parameters that shape a `q` query.
#[derive(Debug, Default, Clone, Copy)]
pub struct UriQuery<'a> {
    /// `q`: the query string.
    pub q: &'a str,
    /// `df`: the field searched when the text names none.
    pub df: Option<&'a str>,
    /// `default_operator`: `and` or `or`.
    pub default_operator: Option<&'a str>,
    /// `analyzer`: the analyzer of the text.
    pub analyzer: Option<&'a str>,
    /// `analyze_wildcard`: `true` or `false`.
    pub analyze_wildcard: Option<&'a str>,
    /// `lenient`: `true` or `false`.
    pub lenient: Option<&'a str>,
}

impl UriQuery<'_> {
    /// The `query_string` query these parameters stand for, as `RestActions.urlParamsToQueryBuilder`
    /// builds it.
    ///
    /// # Errors
    /// A `df`, `default_operator` or `analyzer` the `query_string` query refuses, or a flag that is
    /// neither `true` nor `false`.
    pub fn to_query(&self) -> Result<Query, DslError> {
        let mut body = String::from("{\"query\":{\"query_string\":{\"query\":");
        push_string(&mut body, self.q);
        for (key, value) in [
            ("default_field", self.df),
            ("default_operator", self.default_operator),
            ("analyzer", self.analyzer),
        ] {
            if let Some(value) = value {
                body.push_str(",\"");
                body.push_str(key);
                body.push_str("\":");
                push_string(&mut body, value);
            }
        }
        for (key, value) in [
            ("analyze_wildcard", self.analyze_wildcard),
            ("lenient", self.lenient),
        ] {
            if let Some(value) = value {
                body.push_str(",\"");
                body.push_str(key);
                body.push_str("\":");
                body.push_str(flag(key, value)?);
            }
        }
        body.push_str("}}}");
        parse_chunks([body.as_bytes()])?
            .query
            .ok_or_else(|| DslError::illegal("the [q] parameter built no query"))
    }
}

/// `true` or `false` as `RestRequest.paramAsBoolean` allows.
fn flag<'a>(key: &str, value: &'a str) -> Result<&'a str, DslError> {
    match value {
        "true" | "false" => Ok(value),
        // ALLOC: error path; one message.
        _ => Err(DslError::illegal(format!(
            "Failed to parse value [{value}] as only [true] or [false] are allowed. [{key}]"
        ))),
    }
}

/// Appends `text` as a JSON string.
fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                // ALLOC: one escape of a control character.
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
