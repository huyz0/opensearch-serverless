//! The body of `_rank_eval`: `RankEvalSpec`, `RatedRequest`, `RatedDocument`
//! and the metrics' parsers (modules/rank-eval/src/main/java/org/opensearch/index/rankeval/)
//! at OpenSearch 3.10.0, read from a recorded body.

use std::collections::HashSet;

use super::metric::{DocKey, Metric};
use super::metric_spec::metric;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser, Recorder};
use crate::template::{Val, read_val};
use oss_rest::json::Location;

/// A rated search request.
#[derive(Debug, PartialEq)]
pub struct RatedRequest {
    /// The id the response reports it under.
    pub id: String,
    /// The search body as compact JSON, when given inline.
    pub body: Option<String>,
    /// The template to render, when given by id.
    pub template_id: Option<String>,
    /// The template's params.
    pub params: Val,
    /// The ratings, each document once.
    pub ratings: Vec<(DocKey, i32)>,
}

/// A body of `_rank_eval`.
#[derive(Debug, PartialEq)]
pub struct RankEvalSpec {
    /// The rated requests, in order.
    pub requests: Vec<RatedRequest>,
    /// The metric.
    pub metric: Metric,
    /// The templates by id: their mustache source, or the stored script that holds it.
    pub templates: Vec<(String, TemplateText)>,
}

pub(super) fn wrong(p: &Parser<'_>, spec: &str, field: &str, kind: Option<Kind>) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!(
            "[{spec}] {field} doesn't support values of type: {}",
            kind.map_or("null", Kind::name)
        ),
        None,
    )
}

pub(super) fn unknown(at: Location, spec: &str, name: &str) -> DslError {
    DslError::xcontent(
        at,
        // ALLOC: error path; one message.
        &format!("[{spec}] unknown field [{name}]"),
        None,
    )
}

fn string(p: &Parser<'_>, spec: &str, field: &str, kind: Option<Kind>) -> Result<String, DslError> {
    if kind != Some(Kind::String) {
        return Err(wrong(p, spec, field, kind));
    }
    // ALLOC: one string of the body, which the recording bounds.
    Ok(p.text().to_owned())
}

pub(super) fn int(
    p: &Parser<'_>,
    spec: &str,
    field: &str,
    kind: Option<Kind>,
) -> Result<i32, DslError> {
    if kind != Some(Kind::Number) {
        return Err(wrong(p, spec, field, kind));
    }
    p.text().parse::<i32>().map_err(|_| {
        DslError::xcontent(
            p.location(),
            // ALLOC: error path; one message.
            &format!("[{spec}] {field} doesn't support values of type: VALUE_NUMBER"),
            None,
        )
    })
}

pub(super) fn expect_object(
    p: &Parser<'_>,
    spec: &str,
    kind: Option<Kind>,
) -> Result<(), DslError> {
    if kind == Some(Kind::StartObject) {
        return Ok(());
    }
    Err(DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!(
            "[{spec}] Expected START_OBJECT but was: {}",
            kind.map_or("null", Kind::name)
        ),
        None,
    ))
}

fn missing(p: &Parser<'_>, spec: &str, field: &str) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!("[{spec}] {field} is missing"),
        None,
    )
}

/// A rating: `{_index, _id, rating}`.
fn rating(p: &mut Parser<'_>) -> Result<(DocKey, i32), DslError> {
    expect_object(p, "rated_document", p.current_token())?;
    let (mut index, mut id, mut rating) = (None, None, None);
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: one name of the body, which the recording bounds.
        let name = p.text().to_owned();
        let at = p.location();
        let value = p.next_token();
        match name.as_str() {
            "_index" => index = Some(string(p, "rated_document", &name, value)?),
            "_id" => id = Some(string(p, "rated_document", &name, value)?),
            "rating" => rating = Some(int(p, "rated_document", &name, value)?),
            _ => return Err(unknown(at, "rated_document", &name)),
        }
    }
    let index = index.ok_or_else(|| missing(p, "rated_document", "_index"))?;
    let id = id.ok_or_else(|| missing(p, "rated_document", "_id"))?;
    let rating = rating.ok_or_else(|| missing(p, "rated_document", "rating"))?;
    if index.is_empty() {
        return Err(DslError::illegal(
            "Index must be set for each rated document",
        ));
    }
    if id.is_empty() {
        return Err(DslError::illegal(
            "DocId must be set for each rated document",
        ));
    }
    Ok((DocKey { index, id }, rating))
}

/// A compact JSON text of the object the parser stands on.
fn object_json(p: &mut Parser<'_>) -> Result<String, DslError> {
    // ALLOC: one request body of the request, which the recording bounds.
    let mut json = String::new();
    read_val(p, 0)?.write_json(&mut json);
    Ok(json)
}

fn rated_request(p: &mut Parser<'_>) -> Result<RatedRequest, DslError> {
    expect_object(p, "request", p.current_token())?;
    let mut id = None;
    let mut ratings: Option<Vec<(DocKey, i32)>> = None;
    let mut body = None;
    // ALLOC: the request body and its response, which the limits bound.
    let mut params = Val::Map(Vec::new());
    let mut template_id = None;
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: one name of the body, which the recording bounds.
        let name = p.text().to_owned();
        let at = p.location();
        let value = p.next_token();
        match name.as_str() {
            "id" => id = Some(string(p, "request", &name, value)?),
            "template_id" => template_id = Some(string(p, "request", &name, value)?),
            "request" => {
                expect_object(p, "request", value)?;
                body = Some(object_json(p)?);
            }
            "params" => {
                expect_object(p, "request", value)?;
                params = read_val(p, 0)?;
            }
            "summary_fields" => {
                if value != Some(Kind::StartArray) {
                    return Err(wrong(p, "request", &name, value));
                }
                if p.next_token() != Some(Kind::EndArray) {
                    return Err(DslError::unsupported("a rated request's [summary_fields]"));
                }
            }
            "ratings" => {
                if value != Some(Kind::StartArray) {
                    return Err(wrong(p, "request", &name, value));
                }
                // ALLOC: one entry a rating, which the recording bounds.
                let mut list = Vec::new();
                while p.next_token() == Some(Kind::StartObject) {
                    list.push(rating(p)?);
                }
                ratings = Some(list);
            }
            _ => return Err(unknown(at, "request", &name)),
        }
    }
    let id = id.ok_or_else(|| missing(p, "request", "id"))?;
    let ratings = ratings.ok_or_else(|| missing(p, "request", "ratings"))?;
    let has_params = matches!(&params, Val::Map(m) if !m.is_empty());
    if (has_params && body.is_some()) || (template_id.is_some() && body.is_some()) {
        return Err(DslError::illegal(
            "Ambiguous rated request: Set both, verbatim test request and test request template parameters.",
        ));
    }
    if !has_params && body.is_none() {
        return Err(DslError::illegal(
            "Need to set at least test request or test request template parameters.",
        ));
    }
    if has_params && template_id.is_none() {
        return Err(DslError::illegal(
            "If template parameters are supplied need to set id of template to apply them to too.",
        ));
    }
    // ALLOC: one entry a rating, which the recording bounds.
    let mut seen = HashSet::new();
    for (key, _) in &ratings {
        if !seen.insert(key) {
            // ALLOC: the request body and its response, which the limits bound.
            return Err(DslError::illegal(format!(
                "Found duplicate rated document key [{{\"_index\":\"{}\",\"_id\":\"{}\"}}] in evaluation request [{id}]",
                key.index, key.id
            )));
        }
    }
    Ok(RatedRequest {
        id,
        body,
        template_id,
        params,
        ratings,
    })
}

/// Where a template's mustache source comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateText {
    /// The source written in the body.
    Inline(String),
    /// The id of a stored script holding the source.
    Stored(String),
}

/// A template: `{"id": .., "template": {"source": ..}}` or `{"template": {"id": ..}}`.
fn template(p: &mut Parser<'_>) -> Result<(String, TemplateText), DslError> {
    expect_object(p, "script_with_id", p.current_token())?;
    let (mut id, mut source, mut stored) = (None, None, None);
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: one name of the body, which the recording bounds.
        let name = p.text().to_owned();
        let at = p.location();
        let value = p.next_token();
        match name.as_str() {
            "id" => id = Some(string(p, "script_with_id", &name, value)?),
            "template" => match value {
                Some(Kind::String) => source = Some(string(p, "script", &name, value)?),
                Some(Kind::StartObject) => {
                    while p.next_token() == Some(Kind::FieldName) {
                        // ALLOC: one name of the body, which the recording bounds.
                        let field = p.text().to_owned();
                        let field_at = p.location();
                        let v = p.next_token();
                        match (field.as_str(), v) {
                            ("source" | "inline", Some(Kind::String)) => {
                                source = Some(string(p, "script", &field, v)?);
                            }
                            ("source" | "inline", Some(Kind::StartObject)) => {
                                source = Some(object_json(p)?);
                            }
                            ("lang", Some(Kind::String)) if p.text() == "mustache" => {}
                            ("id", Some(Kind::String)) => {
                                stored = Some(string(p, "script", &field, v)?);
                            }
                            ("lang" | "id" | "params" | "options", _) => {
                                // ALLOC: the request body and its response, which the limits bound.
                                return Err(DslError::unsupported(&format!(
                                    "a template's [{field}]"
                                )));
                            }
                            _ => return Err(unknown(field_at, "script", &field)),
                        }
                    }
                }
                _ => return Err(wrong(p, "script_with_id", &name, value)),
            },
            _ => return Err(unknown(at, "script_with_id", &name)),
        }
    }
    let id = id.ok_or_else(|| missing(p, "script_with_id", "id"))?;
    let text = match (source, stored) {
        (Some(_), Some(_)) => {
            // ALLOC: error path; one message.
            return Err(DslError::illegal(
                "a template takes only one of [source] and [id]".to_owned(),
            ));
        }
        (Some(source), None) => TemplateText::Inline(source),
        (None, Some(stored)) => TemplateText::Stored(stored),
        (None, None) => return Err(missing(p, "script_with_id", "template")),
    };
    Ok((id, text))
}

impl RankEvalSpec {
    /// Parses a recorded body.
    ///
    /// # Errors
    ///
    /// [`DslError`] for a body `RankEvalSpec.parse` refuses, or one this node
    /// does not serve yet.
    pub fn parse(rec: &Recorder) -> Result<Self, DslError> {
        if rec.is_empty() {
            return Err(DslError::illegal(
                "request body or source parameter is required",
            ));
        }
        let mut p = rec.parser();
        let first = p.next_token();
        expect_object(&p, "rank_eval", first)?;
        let mut requests: Option<Vec<RatedRequest>> = None;
        let mut metric_found = None;
        // ALLOC: one entry a template of the body, which the recording bounds.
        let mut templates = Vec::new();
        while p.next_token() == Some(Kind::FieldName) {
            // ALLOC: one name of the body, which the recording bounds.
            let name = p.text().to_owned();
            let at = p.location();
            let value = p.next_token();
            match name.as_str() {
                "requests" => {
                    if value != Some(Kind::StartArray) {
                        return Err(wrong(&p, "rank_eval", &name, value));
                    }
                    // ALLOC: one entry a request of the body, which the recording bounds.
                    let mut list = Vec::new();
                    while p.next_token() == Some(Kind::StartObject) {
                        list.push(rated_request(&mut p)?);
                    }
                    requests = Some(list);
                }
                "metric" => metric_found = Some(metric(&mut p)?),
                "templates" => {
                    if value != Some(Kind::StartArray) {
                        return Err(wrong(&p, "rank_eval", &name, value));
                    }
                    while p.next_token() == Some(Kind::StartObject) {
                        templates.push(template(&mut p)?);
                    }
                }
                "max_concurrent_searches" => {
                    int(&p, "rank_eval", &name, value)?;
                }
                _ => return Err(unknown(at, "rank_eval", &name)),
            }
        }
        let requests = requests.ok_or_else(|| missing(&p, "rank_eval", "requests"))?;
        let metric = metric_found.ok_or_else(|| missing(&p, "rank_eval", "metric"))?;
        if requests.is_empty() {
            return Err(DslError::illegal(
                "Cannot evaluate ranking if no search requests with rated results are provided. Seen: []",
            ));
        }
        for request in &requests {
            let known = request
                .template_id
                .as_deref()
                .is_some_and(|t| templates.iter().any(|(id, _)| id == t));
            if request.body.is_none() && !known {
                if templates.is_empty() {
                    // ALLOC: the request body and its response, which the limits bound.
                    return Err(DslError::illegal(format!(
                        "Cannot evaluate ranking if neither template nor evaluation request is provided. Seen for request id: {}",
                        request.id
                    )));
                }
                // ALLOC: the request body and its response, which the limits bound.
                return Err(DslError::illegal(format!(
                    "no template [{}] for rated request [{}]",
                    request.template_id.as_deref().unwrap_or(""),
                    request.id
                )));
            }
        }
        Ok(Self {
            requests,
            metric,
            templates,
        })
    }
}
