//! `GET|POST [/{index}]/_analyze`: the tokens an analyzer makes of a text.
//!
//! Ported from `RestAnalyzeAction`, `AnalyzeAction` and
//! `TransportAnalyzeAction.simpleAnalyze`
//! (server/src/main/java/org/opensearch/rest/action/admin/indices/RestAnalyzeAction.java,
//! server/src/main/java/org/opensearch/action/admin/indices/analyze/)
//! at OpenSearch 3.10.0 for the `text` and `analyzer` keys over the
//! `standard` and `keyword` analyzers, and for the name of an analyzer the
//! index pins, which runs the module and never falls back (`_analyze` of
//! docs/specs/extension-analyzer).

use std::fmt::Write as _;
use std::sync::Arc;

use lucene_analysis::{Analyzer, TokenStream};
use oss_blob::BlobStore;
use oss_reader::dsl::tokens::Kind;
use oss_rest::{Exception, JSON, Method, Request, Response, RestError, RouteError, Router};

use super::{Answer, SearchService, record_body};

/// `index.analyze.max_token_count`'s default.
const MAX_TOKENS: usize = 10_000;

/// The position gap of an analyzer an index holds: the mapping default.
const INDEX_POSITION_GAP: i32 = 100;

/// Built-in analyzers of OpenSearch this node does not have.
const LACKING: &[&str] = &[
    "simple",
    "whitespace",
    "stop",
    "classic",
    "pattern",
    "fingerprint",
];

/// The routes of `_analyze`, with and without an index, by GET and POST.
pub(super) fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Get, Method::Post] {
        for path in ["/_analyze", "/{index}/_analyze"] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                &[],
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle_analyze(req, resp)
                },
            )?;
        }
    }
    Ok(router)
}

fn not_supported(what: &str) -> RestError {
    RestError::illegal_argument(format!("{what} is not supported by this node yet"))
}

fn validation(reason: &str) -> RestError {
    RestError::exception(
        400,
        Exception::new(
            "action_request_validation_exception",
            format!("Validation Failed: 1: {reason};"),
        ),
    )
}

/// Appends `s` as a JSON string.
fn push_str(out: &mut String, s: &str) {
    oss_doc::mapping::json::write_str(out, s);
}

/// What `_analyze` asked for.
struct Asked<'r> {
    texts: Vec<&'r str>,
    analyzer: Option<&'r str>,
}

fn asked(rec: &oss_reader::dsl::Recorder) -> Result<Asked<'_>, RestError> {
    let mut asked = Asked {
        texts: Vec::new(),
        analyzer: None,
    };
    if rec.is_empty() {
        return Ok(asked);
    }
    let mut p = rec.parser();
    if p.next_token() != Some(Kind::StartObject) {
        return Err(RestError::illegal_argument(
            "an _analyze body must be a JSON object",
        ));
    }
    while p.next_token() == Some(Kind::FieldName) {
        let key = p.text();
        match (key, p.next_token()) {
            ("text", Some(Kind::String)) => asked.texts.push(p.text()),
            ("text", Some(Kind::StartArray)) => {
                while let Some(kind) = p.next_token() {
                    match kind {
                        Kind::String => asked.texts.push(p.text()),
                        Kind::EndArray => break,
                        _ => {
                            return Err(RestError::illegal_argument(
                                "[text] must be a string or an array of strings",
                            ));
                        }
                    }
                }
            }
            ("analyzer", Some(Kind::String)) => asked.analyzer = Some(p.text()),
            ("text" | "analyzer", _) => {
                return Err(RestError::illegal_argument(format!(
                    "[{key}] has the wrong type"
                )));
            }
            (other, _) => {
                return Err(not_supported(&format!("the _analyze key [{other}]")));
            }
        }
    }
    Ok(asked)
}

/// Whether the open index pins an analyzer called `name`.
fn pins<S: BlobStore + 'static>(held: &super::Held<S>, name: &str) -> bool {
    held.pinned
        .as_ref()
        .is_some_and(|p| p.modules().iter().any(|(n, _)| n == name))
}

/// A pin's refusal as a search answers it: retryable, never the built-in
/// analyzer's tokens.
fn pin_refusal(why: String) -> RestError {
    RestError::exception(503, Exception::new("extension_unavailable", why))
}

/// The response being written and where in it the next token sits.
struct Emitter {
    out: String,
    last_position: i32,
    last_offset: i32,
    count: usize,
    gap: i32,
}

impl Emitter {
    /// Appends every token of one text from `stream`, then moves the
    /// position and offset past the text.
    fn stream(
        &mut self,
        stream: &mut dyn TokenStream,
        body: &mut oss_rest::Body<'_>,
    ) -> Result<(), RestError> {
        let failed = |e: lucene_analysis::AnalysisError| {
            RestError::exception(
                500,
                Exception::new("exception", format!("failed to analyze: {e}")),
            )
        };
        stream.reset().map_err(failed)?;
        while stream.increment_token().map_err(failed)? {
            let a = stream.attributes();
            let increment = a.position_increment();
            if increment > 0 {
                self.last_position = self.last_position.saturating_add(increment);
            }
            self.count = self.count.saturating_add(1);
            if self.count > MAX_TOKENS {
                return Err(RestError::exception(
                    500,
                    Exception::new(
                        "illegal_state_exception",
                        format!(
                            "The number of tokens produced by calling _analyze has exceeded the allowed maximum of [{MAX_TOKENS}]. This limit can be set by changing the [index.analyze.max_token_count] index level setting."
                        ),
                    ),
                ));
            }
            body.reserve(a.term().len().saturating_add(160))?;
            if self.count > 1 {
                self.out.push(',');
            }
            self.out.push_str("{\"token\":");
            push_str(&mut self.out, a.term());
            let _ = write!(
                self.out,
                ",\"start_offset\":{},\"end_offset\":{},\"type\":",
                self.last_offset.saturating_add(a.start_offset()),
                self.last_offset.saturating_add(a.end_offset()),
            );
            push_str(&mut self.out, a.token_type());
            let _ = write!(self.out, ",\"position\":{}", self.last_position);
            if a.position_length() > 1 {
                let _ = write!(self.out, ",\"positionLength\":{}", a.position_length());
            }
            self.out.push('}');
        }
        stream.end().map_err(failed)?;
        let a = stream.attributes();
        self.last_offset = self.last_offset.saturating_add(a.end_offset());
        self.last_position = self
            .last_position
            .saturating_add(a.position_increment())
            .saturating_add(self.gap);
        Ok(())
    }
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Handles one `_analyze`.
    pub fn handle_analyze(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let rec = match record_body(req) {
            Ok(rec) => rec,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return resp.send(e.status, JSON, &e.to_body()),
        };
        let asked = asked(&rec)?;
        let index = req.path_param("index");
        if asked.texts.is_empty() {
            return Err(validation("text is missing"));
        }
        let mut held = None;
        let analyzer = match (asked.analyzer, index.is_some()) {
            (None | Some("standard"), _) => Some(Analyzer::standard(None)),
            (Some("keyword"), _) => Some(Analyzer::keyword()),
            (Some(name), _) if LACKING.contains(&name) => {
                return Err(not_supported(&format!("the [{name}] analyzer")));
            }
            (Some(name), false) => {
                return Err(RestError::illegal_argument(format!(
                    "failed to find global analyzer [{name}]"
                )));
            }
            // An index's own pin is not found by name until its shard is
            // open and holds the module.
            (Some(name), true) => {
                match self.open_shard(index.as_deref().unwrap_or_default(), 0, false) {
                    Ok(h) if pins(&h, name) => {
                        held = Some(h);
                        None
                    }
                    Ok(_) => {
                        return Err(RestError::illegal_argument(format!(
                            "failed to find analyzer [{name}]"
                        )));
                    }
                    Err(Answer::Rest(e)) => return Err(e),
                    Err(Answer::Search(e)) => return resp.send(e.status, JSON, &e.to_body()),
                }
            }
        };
        if let (Some(index), None) = (&index, &held) {
            match self.open_shard(index, 0, false) {
                Ok(_) => {}
                Err(Answer::Rest(e)) => return Err(e),
                Err(Answer::Search(e)) => return resp.send(e.status, JSON, &e.to_body()),
            }
        }
        let gap = if index.is_some() {
            INDEX_POSITION_GAP
        } else {
            0
        };
        let body = req.body();
        // ALLOC: the response, each token's bytes reserved as it is added.
        let mut out = Emitter {
            out: String::from("{\"tokens\":["),
            last_position: -1,
            last_offset: 0,
            count: 0,
            gap,
        };
        let failed = |e: lucene_analysis::AnalysisError| {
            RestError::exception(
                500,
                Exception::new("exception", format!("failed to analyze: {e}")),
            )
        };
        for text in &asked.texts {
            if let Some(analyzer) = &analyzer {
                let mut stream = analyzer.token_stream("", text).map_err(failed)?;
                out.stream(&mut stream, body)?;
                stream.close().map_err(failed)?;
                out.last_offset = out.last_offset.saturating_add(analyzer.offset_gap());
            } else if let (Some(held), Some(name)) = (&held, asked.analyzer) {
                // A pin never falls back: a refusal is the search's own.
                let tokens = held
                    .pinned
                    .as_deref()
                    .map_or_else(
                        || {
                            Err(format!(
                                "extension_unavailable: no analyzer [{name}] is loaded"
                            ))
                        },
                        |p| p.tokens(name, text),
                    )
                    .map_err(pin_refusal)?;
                let mut stream = oss_reader::pinned::ListStream::new(tokens);
                out.stream(&mut stream, body)?;
            }
        }
        let mut out = out.out;
        out.push_str("]}");
        resp.send(200, JSON, out.as_bytes())
    }
}
