//! One SQL request: body, statement, names, permission, plan, search, answer.

use oss_query::{Error, Format, Limits, Mapping, Select, Statement};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::{SqlFailure, SqlService};

const TEXT: &str = "text/plain; charset=UTF-8";
/// The permission a statement needs on each index it names.
const ACTION: &str = "indices:data/read/search";

fn send_error(resp: &mut Response<'_>, e: &Error) -> Result<(), RestError> {
    resp.send(e.status(), JSON, e.to_json().to_string().as_bytes())
}

fn failure(resp: &mut Response<'_>, f: SqlFailure) -> Result<(), RestError> {
    match f {
        SqlFailure::Rest(e) => Err(e),
        SqlFailure::Body(status, body) => resp.send(status, JSON, &body),
    }
}

/// The statement text of a request body: `{"query": "..."}`.
fn statement_of(body: &[u8]) -> Result<String, Error> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| Error::syntax("request body", e.column()))?;
    if v.get("cursor").is_some() {
        return Err(Error::unsupported("cursors are not supported yet"));
    }
    v.get("query")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::syntax("request body has no [query]", 0))
}

fn format_of(req: &Request<'_>) -> Result<Format, Error> {
    match req.param("format").as_deref() {
        None | Some("jdbc") => Ok(Format::Jdbc),
        Some("csv") => Ok(Format::Csv),
        Some("raw") => Ok(Format::Raw),
        Some(other) => Err(Error::unsupported(format!(
            "response format [{other}] is not supported yet"
        ))),
    }
}

impl SqlService {
    fn disabled(resp: &mut Response<'_>) -> Result<(), RestError> {
        let body = json!({"error": {"reason": "Invalid SQL query",
            "details": "plugins.sql.enabled setting is false", "type": "SQLFeatureDisabledException"},
            "status": 400});
        resp.send(400, JSON, body.to_string().as_bytes())
    }

    /// Reads the body (at most `max_request_bytes`, 413 past it).
    fn read(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        let max = self.limits.max_request_bytes;
        let body = req.body();
        // ALLOC: the body so far, at most `max`, reserved below.
        let mut out: Vec<u8> = Vec::new();
        loop {
            let chunk = body.max_chunk();
            let more = out
                .len()
                .saturating_add(chunk)
                .saturating_sub(body.reserved());
            body.reserve(more)?;
            let Some(next) = body.next_chunk()? else {
                break;
            };
            if out.len().saturating_add(next.len()) > max {
                return Err(RestError::too_large());
            }
            out.extend_from_slice(next);
        }
        Ok(out)
    }

    /// Checks `names` against the caller's permissions before any search.
    fn permit(&self, req: &Request<'_>, names: &[&str]) -> Result<(), String> {
        self.guard.allow(req.principal(), ACTION, names)
    }

    /// `POST /_plugins/_sql`.
    pub(super) fn run(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.handle(req, resp, false)
    }

    /// `POST /_plugins/_sql/_explain`.
    pub(super) fn explain(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.handle(req, resp, true)
    }

    fn handle(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        explain: bool,
    ) -> Result<(), RestError> {
        if !self.limits.enabled {
            return Self::disabled(resp);
        }
        let bytes = self.read(req)?;
        let format = match format_of(req) {
            Ok(f) => f,
            Err(e) => return send_error(resp, &e),
        };
        let parsed =
            statement_of(&bytes).and_then(|sql| oss_query::parse(&sql, &Limits::default()));
        drop(bytes);
        match parsed {
            Err(e) => send_error(resp, &e),
            Ok(Statement::Select(sel)) => self.select(req, resp, &sel, format, explain),
            Ok(Statement::DescribeTables { pattern, columns }) => {
                self.describe(req, resp, &pattern, columns.as_deref())
            }
            Ok(Statement::ShowTables { .. }) => send_error(
                resp,
                &Error::unsupported("SHOW TABLES is not supported yet"),
            ),
        }
    }

    fn mapping_of(&self, name: &str) -> Result<Mapping, SqlFailure> {
        let json = self.backend.mapping(name)?;
        Mapping::from_json(&json)
            .map_err(|e| SqlFailure::Body(e.status(), e.to_json().to_string().into_bytes()))
    }

    fn describe(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        pattern: &str,
        columns: Option<&str>,
    ) -> Result<(), RestError> {
        // A pattern with a wildcard needs the name listing, a later slice.
        if pattern.contains(['%', '_']) {
            return send_error(
                resp,
                &Error::unsupported("DESCRIBE with a pattern is not supported yet; name one index"),
            );
        }
        if let Err(body) = self.permit(req, &[pattern]) {
            return resp.send(403, JSON, body.as_bytes());
        }
        let mapping = match self.mapping_of(pattern) {
            Ok(m) => m,
            Err(f) => return failure(resp, f),
        };
        match oss_query::describe(
            "serverless",
            pattern,
            &mapping,
            columns,
            self.limits.size_limit,
        ) {
            Ok(text) => resp.send(200, JSON, text.as_bytes()),
            Err(e) => send_error(resp, &e),
        }
    }

    fn select(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        sel: &Select,
        format: Format,
        explain: bool,
    ) -> Result<(), RestError> {
        let names: Vec<String> = sel.index_names().to_vec();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        if let Err(body) = self.permit(req, &refs) {
            return resp.send(403, JSON, body.as_bytes());
        }
        let first = names.first().map_or("", String::as_str);
        let mapping = match self.mapping_of(first) {
            Ok(m) => m,
            Err(f) => return failure(resp, f),
        };
        let plan = match oss_query::plan(sel, &mapping, self.limits.size_limit) {
            Ok(p) => p,
            Err(e) => return send_error(resp, &e),
        };
        if explain {
            let out = json!({"root": {"name": "OpenSearchIndexScan",
                "description": {"request": {"index": names, "sourceBuilder": plan.body()}}}});
            return resp.send(200, JSON, out.to_string().as_bytes());
        }
        let answer = match self.backend.search(&names, plan.body()) {
            Ok(a) => a,
            Err(f) => return failure(resp, f),
        };
        // The parsed answer and the written result each hold about the answer's size.
        req.body().reserve(answer.len().saturating_mul(2))?;
        let Ok(parsed) = serde_json::from_slice::<Value>(&answer) else {
            return Err(RestError::illegal_argument(
                "a search answer of this node could not be read back",
            ));
        };
        drop(answer);
        let max = req.body().reserved().max(1 << 20);
        let text = oss_query::rows(&plan, &parsed)
            .and_then(|rows| oss_query::write(format, &plan, &rows, max));
        match text {
            Ok(t) => resp.send(
                200,
                if matches!(format, Format::Jdbc) {
                    JSON
                } else {
                    TEXT
                },
                t.as_bytes(),
            ),
            Err(e) => send_error(resp, &e),
        }
    }
}
