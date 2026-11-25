//! A forwarded search's decoded source as the search source the reader
//! runs: the query as the JSON a REST search would carry, read by the same
//! parser, so a query means here what it means on a REST search.

use oss_reader::dsl::{DslError, MAX_RECORDED_BYTES, Recorder, SearchSource, source};
use oss_transport::actions::search_shard::{Node, Query, Source, Value};
use serde_json::{Map, Number, Value as Json, json};

/// Why a decoded source could not become a search source.
#[derive(Debug)]
pub enum ConvertError {
    /// A number JSON cannot hold (not finite).
    Number(&'static str),
    /// A named query: the answer would have to carry `matched_queries`,
    /// which the forwarded answer does not.
    Named,
    /// The reader's parser refused what was built.
    Parse(DslError),
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(what) => write!(f, "{what} is not a finite number"),
            Self::Named => write!(f, "a named query is not supported by this node"),
            Self::Parse(e) => write!(f, "{}", e.clone().into_failure().reason),
        }
    }
}

fn number(value: f64, what: &'static str) -> Result<Json, ConvertError> {
    Number::from_f64(value)
        .map(Json::Number)
        .ok_or(ConvertError::Number(what))
}

fn value(v: &Value) -> Result<Json, ConvertError> {
    Ok(match v {
        Value::Null => Json::Null,
        Value::Str(s) => Json::String(s.clone()), // ALLOC: one string, at most the request's bytes
        Value::Int(n) => json!(n),
        Value::Long(n) => json!(n),
        // A float widens the way Java's `Float.toString` text parses back.
        Value::Float(f) => number(
            f.to_string().parse::<f64>().unwrap_or(f64::NAN),
            "a float value",
        )?,
        Value::Double(d) => number(*d, "a double value")?,
        Value::Bool(b) => json!(b),
    })
}

fn boost(f: f32) -> Result<Json, ConvertError> {
    number(f.to_string().parse::<f64>().unwrap_or(f64::NAN), "a boost")
}

fn nodes(list: &[Node]) -> Result<Json, ConvertError> {
    list.iter()
        .map(node)
        .collect::<Result<Vec<_>, _>>()
        .map(Json::Array)
}

fn node(n: &Node) -> Result<Json, ConvertError> {
    let (kind, mut body) = match &n.query {
        Query::MatchAll => ("match_all", Map::new()),
        Query::Term {
            field,
            value: v,
            case_insensitive,
        } => {
            let mut inner = Map::new();
            inner.insert("value".to_owned(), value(v)?);
            if *case_insensitive {
                inner.insert("case_insensitive".to_owned(), json!(true));
            }
            let mut outer = Map::new();
            outer.insert(field.clone(), Json::Object(inner));
            ("term", outer)
        }
        Query::Terms { field, values } => {
            let mut outer = Map::new();
            outer.insert(
                field.clone(),
                Json::Array(values.iter().map(value).collect::<Result<_, _>>()?),
            );
            ("terms", outer)
        }
        Query::Range {
            field,
            from,
            to,
            include_lower,
            include_upper,
            time_zone,
            format,
            relation,
        } => {
            let mut inner = Map::new();
            if *from != Value::Null {
                inner.insert(
                    if *include_lower { "gte" } else { "gt" }.to_owned(),
                    value(from)?,
                );
            }
            if *to != Value::Null {
                inner.insert(
                    if *include_upper { "lte" } else { "lt" }.to_owned(),
                    value(to)?,
                );
            }
            for (key, text) in [
                ("time_zone", time_zone),
                ("format", format),
                ("relation", relation),
            ] {
                if let Some(text) = text {
                    inner.insert(key.to_owned(), json!(text));
                }
            }
            let mut outer = Map::new();
            outer.insert(field.clone(), Json::Object(inner));
            ("range", outer)
        }
        Query::Bool {
            must,
            must_not,
            should,
            filter,
            adjust_pure_negative,
            minimum_should_match,
        } => {
            let mut body = Map::new();
            for (key, list) in [
                ("must", must),
                ("must_not", must_not),
                ("should", should),
                ("filter", filter),
            ] {
                if !list.is_empty() {
                    body.insert(key.to_owned(), nodes(list)?);
                }
            }
            body.insert(
                "adjust_pure_negative".to_owned(),
                json!(adjust_pure_negative),
            );
            if let Some(m) = minimum_should_match {
                body.insert("minimum_should_match".to_owned(), json!(m));
            }
            ("bool", body)
        }
        Query::Ids(ids) => {
            let mut body = Map::new();
            body.insert("values".to_owned(), json!(ids));
            ("ids", body)
        }
        Query::Exists(field) => {
            let mut body = Map::new();
            body.insert("field".to_owned(), json!(field));
            ("exists", body)
        }
    };
    // `boost` and `_name` sit beside the field of a field query, in the
    // field's object, and in the body of any other.
    let carries_field = matches!(n.query, Query::Term { .. } | Query::Range { .. });
    let holder = if carries_field {
        body.values_mut().next().and_then(Json::as_object_mut)
    } else {
        Some(&mut body)
    };
    if let Some(holder) = holder
        && (n.boost - 1.0).abs() > f32::EPSILON
    {
        holder.insert("boost".to_owned(), boost(n.boost)?);
    }
    if n.name.is_some() {
        return Err(ConvertError::Named);
    }
    Ok(json!({ kind: Json::Object(body) }))
}

/// `source` as the JSON of a search body. Every shard's own size and sort are
/// the request's: `_source` is fetched with the document's version, sequence
/// number and primary term so the answer carries them.
fn body(source: &Source) -> Result<Json, ConvertError> {
    let mut top = Map::new();
    if let Some(q) = &source.query {
        top.insert("query".to_owned(), node(q)?);
    }
    if source.from >= 0 {
        top.insert("from".to_owned(), json!(source.from));
    }
    if source.size >= 0 {
        top.insert("size".to_owned(), json!(source.size));
    }
    if let Some(m) = source.min_score {
        top.insert("min_score".to_owned(), boost(m)?);
    }
    if source.terminate_after > 0 {
        top.insert("terminate_after".to_owned(), json!(source.terminate_after));
    }
    if let Some(up_to) = source.track_total_hits_up_to {
        // Java's sentinels: the largest int counts exactly, -1 not at all.
        top.insert(
            "track_total_hits".to_owned(),
            match up_to {
                i32::MAX => json!(true),
                -1 => json!(false),
                n => json!(n),
            },
        );
    }
    if let Some((fetch, includes, excludes)) = &source.fetch_source {
        top.insert(
            "_source".to_owned(),
            if !fetch {
                json!(false)
            } else if includes.is_empty() && excludes.is_empty() {
                json!(true)
            } else {
                json!({ "includes": includes, "excludes": excludes })
            },
        );
    }
    top.insert("version".to_owned(), json!(true));
    top.insert("seq_no_primary_term".to_owned(), json!(true));
    Ok(Json::Object(top))
}

/// The search source `source` stands for.
///
/// # Errors
///
/// [`ConvertError`] for a number JSON cannot hold, or a source the reader's
/// parser refuses.
pub fn search_source(source: &Source) -> Result<SearchSource, ConvertError> {
    let text = serde_json::to_vec(&body(source)?).map_err(|_| ConvertError::Number("the body"))?;
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokenizer = source::tokenizer();
    tokenizer
        .feed(&text, &mut rec)
        .map_err(ConvertError::Parse)?;
    tokenizer.finish(&mut rec).map_err(ConvertError::Parse)?;
    SearchSource::parse(&rec).map_err(ConvertError::Parse)
}
