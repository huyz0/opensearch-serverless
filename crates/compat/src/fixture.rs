//! The golden-fixture format.
//!
//! A fixture is one JSON file: optional cleanup requests, then steps. Each
//! step holds a request, the response stock OpenSearch gave it, and the
//! normalisation rules for fields that legitimately vary. The format is
//! described for people in `fixtures/compat/README.md`. Parsing is strict:
//! an unknown key is refused, so a misspelt `normalize` cannot silently
//! disable a rule.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::pointer::Pointer;

/// What is wrong with a fixture, and where in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureError {
    /// The location inside the fixture, for example `steps[1].request.method`.
    pub at: String,
    /// What is wrong there.
    pub message: String,
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

impl std::error::Error for FixtureError {}

fn err(at: &str, message: impl Into<String>) -> FixtureError {
    FixtureError {
        at: at.to_owned(),
        message: message.into(),
    }
}

/// An HTTP method the suite replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GET`
    Get,
    /// `PUT`
    Put,
    /// `POST`
    Post,
    /// `DELETE`
    Delete,
    /// `HEAD`
    Head,
}

impl Method {
    /// The method's name on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Get, Self::Put, Self::Post, Self::Delete, Self::Head]
            .into_iter()
            .find(|m| m.as_str() == text)
    }
}

/// A request body.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// No body.
    None,
    /// One JSON document, sent as `application/json`.
    Json(Value),
    /// One JSON document per line, sent as `application/x-ndjson` (bulk,
    /// msearch).
    Ndjson(Vec<Value>),
}

/// A request to replay.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The HTTP method.
    pub method: Method,
    /// The path, starting with `/`, without a query string.
    pub path: String,
    /// Query parameters, sent in name order.
    pub query: BTreeMap<String, String>,
    /// The body.
    pub body: Body,
}

impl Request {
    /// The `Content-Type` this request is sent with, if it has a body.
    pub fn content_type(&self) -> Option<&'static str> {
        match self.body {
            Body::None => None,
            Body::Json(_) => Some("application/json"),
            Body::Ndjson(_) => Some("application/x-ndjson"),
        }
    }

    /// The body as sent on the wire: compact JSON, or one compact document
    /// per line with a trailing newline (bulk requires it).
    pub fn encode_body(&self) -> Option<Vec<u8>> {
        match &self.body {
            Body::None => None,
            Body::Json(doc) => Some(doc.to_string().into_bytes()),
            Body::Ndjson(lines) => {
                let mut out = Vec::new();
                for line in lines {
                    out.extend_from_slice(line.to_string().as_bytes());
                    out.push(b'\n');
                }
                Some(out)
            }
        }
    }
}

/// A response: recorded from OpenSearch in a fixture, or returned by a
/// target.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// Headers by lower-cased name. In a fixture, only the headers the
    /// recording selected; they are the only ones compared.
    pub headers: BTreeMap<String, String>,
    /// The JSON body, or `None` when there is none (HEAD, some 404s).
    pub body: Option<Value>,
}

impl Response {
    /// The document normalisation rules and the comparator see:
    /// `{"status": .., "headers": {..}, "body": ..}`. When `select` is given,
    /// only those headers are kept.
    pub fn to_document(&self, select: Option<&BTreeMap<String, String>>) -> Value {
        let headers: Map<String, Value> = self
            .headers
            .iter()
            .filter(|(name, _)| select.is_none_or(|keep| keep.contains_key(*name)))
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect();
        let mut doc = Map::new();
        doc.insert("status".to_owned(), Value::from(self.status));
        doc.insert("headers".to_owned(), Value::Object(headers));
        doc.insert("body".to_owned(), self.body.clone().unwrap_or(Value::Null));
        Value::Object(doc)
    }
}

/// A rule for a field that legitimately varies between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Where the field is, in the response document (`/body/took`).
    pub pointer: Pointer,
    /// Why it varies. Required: a rule nobody can justify hides a difference.
    pub reason: String,
}

/// One request and what OpenSearch answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// A short name for reports.
    pub name: String,
    /// The request.
    pub request: Request,
    /// The recorded response; `None` until the recording script has run.
    pub response: Option<Response>,
    /// Normalisation rules for this step's response.
    pub normalise: Vec<Rule>,
}

/// Which OpenSearch build a fixture was recorded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedFrom {
    /// `version.number` from `GET /`.
    pub version: String,
    /// `version.build_hash` from `GET /`.
    pub build_hash: String,
}

/// A parsed fixture.
#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    /// A short name for reports.
    pub name: String,
    /// The build the responses came from; `None` until recorded.
    pub recorded_from: Option<RecordedFrom>,
    /// Requests sent before the steps (and after, by the recorder) so a
    /// fixture can run against a target that is not fresh. Their responses
    /// are not compared.
    pub cleanup: Vec<Request>,
    /// The steps, in order.
    pub steps: Vec<Step>,
}

impl Fixture {
    /// Parses a fixture from its JSON text.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, FixtureError> {
        let doc: Value = serde_json::from_slice(bytes).map_err(|e| err("$", e.to_string()))?;
        Self::from_value(&doc)
    }

    /// Parses a fixture from a JSON document.
    pub fn from_value(doc: &Value) -> Result<Self, FixtureError> {
        let obj = object(
            doc,
            "$",
            &["name", "description", "recorded_from", "cleanup", "steps"],
        )?;
        let cleanup = match obj.get("cleanup") {
            None => Vec::new(),
            Some(v) => array(v, "cleanup")?
                .iter()
                .enumerate()
                .map(|(i, r)| parse_request(r, &format!("cleanup[{i}]")))
                .collect::<Result<_, _>>()?,
        };
        let steps: Vec<Step> = array(required(obj, "steps", "$")?, "steps")?
            .iter()
            .enumerate()
            .map(|(i, s)| parse_step(s, &format!("steps[{i}]")))
            .collect::<Result<_, _>>()?;
        if steps.is_empty() {
            return Err(err("steps", "a fixture needs at least one step"));
        }
        if let Some(d) = obj.get("description") {
            string(d, "description")?;
        }
        Ok(Self {
            name: non_empty(required(obj, "name", "$")?, "name")?,
            recorded_from: obj
                .get("recorded_from")
                .map(parse_recorded_from)
                .transpose()?,
            cleanup,
            steps,
        })
    }
}

fn parse_recorded_from(v: &Value) -> Result<RecordedFrom, FixtureError> {
    let at = "recorded_from";
    let obj = object(v, at, &["distribution", "version", "build_hash"])?;
    if let Some(d) = obj.get("distribution") {
        string(d, &format!("{at}.distribution"))?;
    }
    Ok(RecordedFrom {
        version: non_empty(required(obj, "version", at)?, &format!("{at}.version"))?,
        build_hash: non_empty(
            required(obj, "build_hash", at)?,
            &format!("{at}.build_hash"),
        )?,
    })
}

fn parse_step(v: &Value, at: &str) -> Result<Step, FixtureError> {
    let obj = object(v, at, &["name", "request", "response", "normalise"])?;
    let normalise = match obj.get("normalise") {
        None => Vec::new(),
        Some(rules) => array(rules, &format!("{at}.normalise"))?
            .iter()
            .enumerate()
            .map(|(i, r)| parse_rule(r, &format!("{at}.normalise[{i}]")))
            .collect::<Result<_, _>>()?,
    };
    Ok(Step {
        name: non_empty(required(obj, "name", at)?, &format!("{at}.name"))?,
        request: parse_request(required(obj, "request", at)?, &format!("{at}.request"))?,
        response: obj
            .get("response")
            .map(|r| parse_response(r, &format!("{at}.response")))
            .transpose()?,
        normalise,
    })
}

fn parse_request(v: &Value, at: &str) -> Result<Request, FixtureError> {
    let obj = object(v, at, &["method", "path", "query", "body", "ndjson"])?;
    let method_at = format!("{at}.method");
    let method_text = string(required(obj, "method", at)?, &method_at)?;
    let method = Method::parse(method_text)
        .ok_or_else(|| err(&method_at, format!("unknown method {method_text:?}")))?;
    let path_at = format!("{at}.path");
    let path = string(required(obj, "path", at)?, &path_at)?;
    if !path.starts_with('/') || path.contains('?') {
        return Err(err(
            &path_at,
            "must start with '/' and carry no query string (use `query`)",
        ));
    }
    let query = match obj.get("query") {
        None => BTreeMap::new(),
        Some(q) => string_map(q, &format!("{at}.query"), false)?,
    };
    let body = match (obj.get("body"), obj.get("ndjson")) {
        (None, None) => Body::None,
        (Some(b), None) => Body::Json(b.clone()),
        (None, Some(lines)) => Body::Ndjson(array(lines, &format!("{at}.ndjson"))?.clone()),
        (Some(_), Some(_)) => {
            return Err(err(
                at,
                "has both `body` and `ndjson`; a request has one body",
            ));
        }
    };
    Ok(Request {
        method,
        path: path.to_owned(),
        query,
        body,
    })
}

fn parse_response(v: &Value, at: &str) -> Result<Response, FixtureError> {
    let obj = object(v, at, &["status", "headers", "body"])?;
    let status_at = format!("{at}.status");
    let status = required(obj, "status", at)?
        .as_u64()
        .and_then(|s| u16::try_from(s).ok())
        .filter(|s| (100..600).contains(s))
        .ok_or_else(|| err(&status_at, "must be an HTTP status between 100 and 599"))?;
    let headers = match obj.get("headers") {
        None => BTreeMap::new(),
        Some(h) => string_map(h, &format!("{at}.headers"), true)?,
    };
    Ok(Response {
        status,
        headers,
        body: obj.get("body").cloned(),
    })
}

fn parse_rule(v: &Value, at: &str) -> Result<Rule, FixtureError> {
    let obj = object(v, at, &["pointer", "reason"])?;
    let pointer_at = format!("{at}.pointer");
    let pointer = Pointer::parse(string(required(obj, "pointer", at)?, &pointer_at)?)
        .map_err(|m| err(&pointer_at, m))?;
    Ok(Rule {
        pointer,
        reason: non_empty(required(obj, "reason", at)?, &format!("{at}.reason"))?,
    })
}

fn object<'a>(
    v: &'a Value,
    at: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, FixtureError> {
    let obj = v.as_object().ok_or_else(|| err(at, "must be an object"))?;
    if let Some(unknown) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(err(
            at,
            format!("unknown key {unknown:?}; expected one of {allowed:?}"),
        ));
    }
    Ok(obj)
}

fn required<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    at: &str,
) -> Result<&'a Value, FixtureError> {
    obj.get(key)
        .ok_or_else(|| err(at, format!("missing required key {key:?}")))
}

fn array<'a>(v: &'a Value, at: &str) -> Result<&'a Vec<Value>, FixtureError> {
    v.as_array().ok_or_else(|| err(at, "must be an array"))
}

fn string<'a>(v: &'a Value, at: &str) -> Result<&'a str, FixtureError> {
    v.as_str().ok_or_else(|| err(at, "must be a string"))
}

fn non_empty(v: &Value, at: &str) -> Result<String, FixtureError> {
    let s = string(v, at)?;
    if s.trim().is_empty() {
        return Err(err(at, "must not be empty"));
    }
    Ok(s.to_owned())
}

/// A string-to-string object. Header names are lower-cased so a fixture's
/// `Content-Type` and a target's `content-type` are the same header.
fn string_map(
    v: &Value,
    at: &str,
    lower_keys: bool,
) -> Result<BTreeMap<String, String>, FixtureError> {
    let obj = v.as_object().ok_or_else(|| err(at, "must be an object"))?;
    obj.iter()
        .map(|(k, val)| {
            let key = if lower_keys {
                k.to_ascii_lowercase()
            } else {
                k.clone()
            };
            Ok((key, string(val, &format!("{at}.{k}"))?.to_owned()))
        })
        .collect()
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
