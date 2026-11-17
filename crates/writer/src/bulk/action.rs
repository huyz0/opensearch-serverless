//! One action and metadata line, read token by token as
//! `BulkRequestParser.parse` reads it: the action name, then its
//! parameters, with the Java's messages for a line that is none.

use oss_rest::RestError;
use oss_rest::json::{Token, TokenSink};

use crate::index::{Expect, MATCH_ANY, UNASSIGNED_SEQ_NO, VersionType};

/// What an action asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `index`: create or replace.
    Index,
    /// `create`: only an id with no live document.
    Create,
    /// `delete`.
    Delete,
    /// `update`: a partial merge.
    Update,
}

impl Kind {
    /// The name the response keys the item by.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Index => "index",
            Kind::Create => "create",
            Kind::Delete => "delete",
            Kind::Update => "update",
        }
    }
}

/// An action line's metadata, with the request's defaults filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    /// The action.
    pub kind: Kind,
    /// `_index`, or the path's.
    pub index: Option<String>,
    /// `_id`.
    pub id: Option<String>,
    /// `routing`, or the request's.
    pub routing: Option<String>,
    /// `op_type` of an `index` action: `create` makes it one.
    pub create: bool,
    /// `version`, `version_type`, `if_seq_no` and `if_primary_term`.
    pub expect: Expect,
    /// `pipeline`, or the request's.
    pub pipeline: Option<String>,
    /// `require_alias`, or the request's.
    pub require_alias: bool,
}

/// The request's defaults for every action line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    /// The index in the path.
    pub index: Option<String>,
    /// `?routing`.
    pub routing: Option<String>,
    /// `?pipeline`.
    pub pipeline: Option<String>,
    /// `?require_alias`.
    pub require_alias: bool,
}

/// Where the reader is in the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    /// Before the line's first token.
    Start,
    /// After `{`, before the action name.
    Name,
    /// After the action name, before its object.
    Params,
    /// Inside the action's object.
    Inside,
    /// Skipping a `_source` object, this deep.
    Source(u32),
    /// The action's object has closed; the rest of the line is not read.
    Done,
}

/// Reads one action line's tokens.
pub(super) struct ActionReader<'d> {
    line: u64,
    defaults: &'d Defaults,
    at: At,
    field: Option<String>,
    text: String,
    action: Option<Action>,
}

impl<'d> ActionReader<'d> {
    /// A reader for the action on `line`.
    pub(super) fn new(line: u64, defaults: &'d Defaults) -> Self {
        Self {
            line,
            defaults,
            at: At::Start,
            field: None,
            // ALLOC: one string value of an action line; bounded by the tokenizer's limits.
            text: String::new(),
            action: None,
        }
    }

    /// Whether the action's object has closed.
    pub(super) fn is_done(&self) -> bool {
        self.at == At::Done
    }

    /// The action read, once its object has closed.
    pub(super) fn take(&mut self) -> Option<Action> {
        self.action.take()
    }

    fn malformed(&self, expected: &str, found: &str) -> RestError {
        // ALLOC: error path; one message.
        RestError::illegal_argument(format!(
            "Malformed action/metadata line [{}], expected {expected} but found [{found}]",
            self.line
        ))
    }

    fn start(&mut self, name: &str) -> Result<(), RestError> {
        let kind = match name {
            "index" => Kind::Index,
            "create" => Kind::Create,
            "delete" => Kind::Delete,
            "update" => Kind::Update,
            other => {
                // ALLOC: error path; one message.
                return Err(RestError::illegal_argument(format!(
                    "Malformed action/metadata line [{}], expected one of [create, delete, index, update] but found [{other}]",
                    self.line
                )));
            }
        };
        let d = self.defaults;
        self.action = Some(Action {
            kind,
            index: d.index.clone(),
            id: None,
            routing: d.routing.clone(),
            create: kind == Kind::Create,
            expect: Expect {
                version: MATCH_ANY,
                version_type: VersionType::Internal,
                if_seq_no: UNASSIGNED_SEQ_NO,
                if_primary_term: 0,
            },
            pipeline: d.pipeline.clone(),
            require_alias: d.require_alias,
        });
        Ok(())
    }

    /// Sets the current field from a scalar's text.
    fn value(&mut self, text: &str, kind: Scalar) -> Result<(), RestError> {
        let line = self.line;
        let field = self.field.as_deref().unwrap_or("");
        let Some(action) = self.action.as_mut() else {
            return Ok(());
        };
        match field {
            // ALLOC: one name or id a line, bounded by the tokenizer's limits.
            "_index" => action.index = Some(text.to_owned()),
            // ALLOC: as above.
            "_id" => action.id = Some(text.to_owned()),
            // ALLOC: as above.
            "routing" => action.routing = Some(text.to_owned()),
            "op_type" => {
                // `IndexRequest.create("create".equals(opType))`: only an
                // index action reads it.
                if action.kind == Kind::Index {
                    action.create = text == "create";
                }
            }
            "version" => action.expect.version = long(text, kind)?,
            "version_type" => {
                action.expect.version_type = VersionType::parse(text).ok_or_else(|| {
                    // ALLOC: error path; one message.
                    RestError::illegal_argument(format!("No version type match [{text}]"))
                })?;
            }
            "if_seq_no" => action.expect.if_seq_no = long(text, kind)?,
            "if_primary_term" => action.expect.if_primary_term = long(text, kind)?,
            "retry_on_conflict" => {
                long(text, kind)?;
            }
            // ALLOC: as above.
            "pipeline" => action.pipeline = Some(text.to_owned()),
            // A scalar `_source` names fields to return, which a write
            // answer never carries.
            "_source" => {}
            "require_alias" => action.require_alias = boolean(text)?,
            other => {
                // ALLOC: error path; one message.
                return Err(RestError::illegal_argument(format!(
                    "Action/metadata line [{line}] contains an unknown parameter [{other}]"
                )));
            }
        }
        Ok(())
    }

    fn simple_value(&self, found: &str) -> RestError {
        // ALLOC: error path; one message.
        RestError::illegal_argument(format!(
            "Malformed action/metadata line [{}], expected a simple value for field [{}] but found [{found}]",
            self.line,
            self.field.as_deref().unwrap_or("null")
        ))
    }
}

/// How a scalar was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scalar {
    Str,
    Number,
    Bool,
}

/// `XContentParser.longValue()`, which coerces a string and truncates a
/// fraction.
fn long(text: &str, kind: Scalar) -> Result<i64, RestError> {
    if kind == Scalar::Bool {
        // ALLOC: error path; one message.
        return Err(RestError::illegal_argument(format!(
            "Current token (VALUE_{}) not numeric, can not use numeric value accessors",
            if text == "true" { "TRUE" } else { "FALSE" }
        )));
    }
    if let Ok(v) = text.parse::<i64>() {
        return Ok(v);
    }
    let v = crate::mapping::java_parse_double(text)
        .filter(|v| v.is_finite())
        .ok_or_else(|| {
            // ALLOC: error path; one message.
            RestError::illegal_argument(format!("For input string: \"{text}\""))
        })?;
    // `(double) Long.MAX_VALUE` is 2^63.
    if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&v) {
        // ALLOC: error path; one message.
        return Err(RestError::illegal_argument(format!(
            "Value [{text}] is out of range for a long"
        )));
    }
    // In range, so the truncation is the Java's `(long)` cast.
    #[allow(clippy::cast_possible_truncation)]
    Ok(v.trunc() as i64)
}

/// `XContentParser.booleanValue()`: `true` or `false`, written either way.
fn boolean(text: &str) -> Result<bool, RestError> {
    match text {
        "true" => Ok(true),
        "false" => Ok(false),
        other => {
            // ALLOC: error path; one message.
            Err(RestError::illegal_argument(format!(
                "Failed to parse value [{other}] as only [true] or [false] are allowed."
            )))
        }
    }
}

/// The token's name in the Java's messages.
fn token_name(token: &Token<'_>) -> &'static str {
    match token {
        Token::StartObject => "START_OBJECT",
        Token::EndObject => "END_OBJECT",
        Token::StartArray => "START_ARRAY",
        Token::EndArray => "END_ARRAY",
        Token::Key(_) => "FIELD_NAME",
        Token::Str { .. } => "VALUE_STRING",
        Token::Number(_) => "VALUE_NUMBER",
        Token::Bool(_) => "VALUE_BOOLEAN",
        Token::Null => "VALUE_NULL",
    }
}

impl TokenSink for ActionReader<'_> {
    type Error = RestError;

    fn token(&mut self, token: Token<'_>) -> Result<(), RestError> {
        match self.at {
            At::Start => {
                if token != Token::StartObject {
                    return Err(self.malformed("START_OBJECT", token_name(&token)));
                }
                self.at = At::Name;
            }
            At::Name => {
                let Token::Key(name) = token else {
                    return Err(self.malformed("FIELD_NAME", token_name(&token)));
                };
                self.start(name)?;
                self.at = At::Params;
            }
            At::Params => match token {
                Token::StartObject => self.at = At::Inside,
                Token::EndObject => self.at = At::Done,
                other => {
                    return Err(self.malformed("START_OBJECT or END_OBJECT", token_name(&other)));
                }
            },
            At::Inside => match token {
                // ALLOC: one key a parameter, bounded by the tokenizer's key limit.
                Token::Key(k) => self.field = Some(k.to_owned()),
                Token::EndObject => self.at = At::Done,
                Token::Str { text, last } => {
                    self.text.push_str(text);
                    if last {
                        let value = std::mem::take(&mut self.text);
                        self.value(&value, Scalar::Str)?;
                    }
                }
                Token::Number(n) => self.value(n, Scalar::Number)?,
                Token::Bool(b) => self.value(if b { "true" } else { "false" }, Scalar::Bool)?,
                Token::Null => {}
                Token::StartObject if self.field.as_deref() == Some("_source") => {
                    self.at = At::Source(1);
                }
                other => return Err(self.simple_value(token_name(&other))),
            },
            At::Source(depth) => match token {
                Token::StartObject | Token::StartArray => {
                    self.at = At::Source(depth.saturating_add(1));
                }
                Token::EndObject | Token::EndArray => {
                    self.at = if depth <= 1 {
                        At::Inside
                    } else {
                        At::Source(depth.saturating_sub(1))
                    };
                }
                _ => {}
            },
            At::Done => {}
        }
        Ok(())
    }
}
