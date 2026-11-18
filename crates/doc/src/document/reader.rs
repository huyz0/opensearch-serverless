//! A pull reader over one document's bytes, the way `DocumentParser` pulls
//! tokens from Jackson: one token at a time, with where it starts and ends
//! for the messages that name a location.

use std::collections::HashSet;

use chunkjson::{Config, Event, Framing, Tokenizer};

use crate::mapping::MappingError;
use crate::mapping::json::Json;

/// One token, with its text owned.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// `{`
    BeginObject,
    /// `}`
    EndObject,
    /// `[`
    BeginArray,
    /// `]`
    EndArray,
    /// An object key.
    Key(String),
    /// A string value.
    Str(String),
    /// A number, as its JSON text.
    Num(String),
    /// `true` or `false`.
    Bool(bool),
    /// `null`
    Null,
    /// The document ended.
    Eof,
}

impl Tok {
    /// Jackson's name for the token, as its messages print it.
    pub fn jackson_name(&self) -> &'static str {
        match self {
            Tok::BeginObject => "START_OBJECT",
            Tok::EndObject => "END_OBJECT",
            Tok::BeginArray => "START_ARRAY",
            Tok::EndArray => "END_ARRAY",
            Tok::Key(_) => "FIELD_NAME",
            Tok::Str(_) => "VALUE_STRING",
            Tok::Num(t) if crate::mapping::json::is_integral(t) => "VALUE_NUMBER_INT",
            Tok::Num(_) => "VALUE_NUMBER_FLOAT",
            Tok::Bool(true) => "VALUE_TRUE",
            Tok::Bool(false) => "VALUE_FALSE",
            Tok::Null => "VALUE_NULL",
            Tok::Eof => "null",
        }
    }
}

/// Where the current token is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct At {
    /// The 1-based line of its first byte.
    pub line: u64,
    /// The 1-based column of its first byte.
    pub column: u64,
    /// The offset one past its last byte.
    pub end: u64,
}

/// Deepest nesting a document may have, as the tokenizer's default.
const MAX_DEPTH: usize = 128;

/// Reads one document's tokens.
pub struct Reader<'a> {
    tokenizer: Tokenizer,
    input: &'a [u8],
    used: usize,
    at: At,
    /// The keys of each open object (`None` for an open array), for
    /// Jackson's `STRICT_DUPLICATE_DETECTION`, which OpenSearch enables.
    keys: Vec<Option<HashSet<String>>>,
}

impl<'a> Reader<'a> {
    /// A reader over a whole document.
    pub fn new(input: &'a [u8]) -> Self {
        // Concatenated, so a second value after the document is a token
        // (`found extra data`), as Jackson reads one, not a syntax error.
        let config = Config {
            framing: Framing::Concatenated,
            max_depth: MAX_DEPTH,
            ..Config::default()
        };
        Self {
            tokenizer: Tokenizer::new(config),
            input,
            used: 0,
            at: At::default(),
            // ALLOC: one entry a nesting level, at most MAX_DEPTH.
            keys: Vec::new(),
        }
    }

    /// Where the last token read is.
    pub fn at(&self) -> At {
        self.at
    }

    /// The next token.
    ///
    /// # Errors
    ///
    /// A `json_parse_exception` for malformed JSON.
    pub fn next(&mut self) -> Result<Tok, MappingError> {
        // ALLOC: one key or string, bounded by the document's bytes.
        let mut text = String::new();
        let mut first = None;
        loop {
            let rest = self.input.get(self.used..).unwrap_or_default();
            let (event, consumed, pos) = self.tokenizer.next_at(rest, true).map_err(|e| {
                // ALLOC: error path; one message.
                MappingError::json_parse(&format!(
                    "{e}\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: {}, column: {}]",
                    e.line(),
                    e.column()
                ))
            })?;
            let start = *first.get_or_insert(pos);
            let len = |n: usize| pos.offset.saturating_add(n as u64);
            let tok = match event {
                Event::BeginObject => Some((Tok::BeginObject, len(1))),
                Event::EndObject => Some((Tok::EndObject, len(1))),
                Event::BeginArray => Some((Tok::BeginArray, len(1))),
                Event::EndArray => Some((Tok::EndArray, len(1))),
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                Event::Number(n) => Some((Tok::Num(n.to_owned()), len(n.len()))),
                Event::Bool(true) => Some((Tok::Bool(true), len(4))),
                Event::Bool(false) => Some((Tok::Bool(false), len(5))),
                Event::Null => Some((Tok::Null, len(4))),
                Event::Key(t) | Event::String(t) => {
                    text.push_str(t.fragment);
                    if t.last {
                        let end = self.used.saturating_add(consumed) as u64;
                        let done = std::mem::take(&mut text);
                        Some((
                            if matches!(event, Event::Key(_)) {
                                Tok::Key(done)
                            } else {
                                Tok::Str(done)
                            },
                            end,
                        ))
                    } else {
                        None
                    }
                }
                Event::Eof => Some((Tok::Eof, pos.offset)),
                Event::EndDocument { .. } | Event::NeedMore => {
                    first = None;
                    None
                }
            };
            self.used = self.used.saturating_add(consumed);
            if let Some((tok, end)) = tok {
                self.at = At {
                    line: start.line,
                    column: start.column,
                    end,
                };
                self.track(&tok)?;
                return Ok(tok);
            }
        }
    }

    /// Keeps the open objects' keys, refusing a repeated one.
    fn track(&mut self, tok: &Tok) -> Result<(), MappingError> {
        match tok {
            // ALLOC: one set an open object, at most MAX_DEPTH, of the document's keys.
            Tok::BeginObject => self.keys.push(Some(HashSet::new())),
            Tok::BeginArray => self.keys.push(None),
            Tok::EndObject | Tok::EndArray => {
                self.keys.pop();
            }
            Tok::Key(k) => {
                if let Some(Some(seen)) = self.keys.last_mut()
                    && !seen.insert(k.clone())
                {
                    // ALLOC: error path; one message.
                    let reason = format!(
                        "Duplicate Object property \"{k}\"\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); byte offset: #{}]",
                        self.at.end
                    );
                    return Err(MappingError::json_parse(&reason)
                        .caused_by(MappingError::stream_read(&reason)));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Skips the rest of the value `current` opens (`skipChildren`).
    ///
    /// # Errors
    ///
    /// As [`Reader::next`].
    pub fn skip_children(&mut self, current: &Tok) -> Result<(), MappingError> {
        if !matches!(current, Tok::BeginObject | Tok::BeginArray) {
            return Ok(());
        }
        let mut depth = 1usize;
        while depth > 0 {
            match self.next()? {
                Tok::BeginObject | Tok::BeginArray => depth = depth.saturating_add(1),
                Tok::EndObject | Tok::EndArray => depth = depth.saturating_sub(1),
                Tok::Eof => return Err(MappingError::json_parse("unexpected end of input")),
                _ => {}
            }
        }
        Ok(())
    }

    /// Reads the whole value `current` starts, as `readValue` does.
    ///
    /// # Errors
    ///
    /// As [`Reader::next`].
    pub fn read_value(&mut self, current: Tok) -> Result<Json, MappingError> {
        Ok(match current {
            Tok::Str(s) => Json::Str(s),
            Tok::Num(n) => Json::Num(n),
            Tok::Bool(b) => Json::Bool(b),
            Tok::BeginArray => {
                // ALLOC: one preview's items, bounded by the document's bytes; error path.
                let mut items = Vec::new();
                loop {
                    match self.next()? {
                        Tok::EndArray => break,
                        t => items.push(self.read_value(t)?),
                    }
                }
                Json::Arr(items)
            }
            Tok::BeginObject => {
                // ALLOC: one preview's members, bounded by the document's bytes; error path.
                let mut members: Vec<(String, Json)> = Vec::new();
                while let Tok::Key(k) = self.next()? {
                    let t = self.next()?;
                    let v = self.read_value(t)?;
                    match members.iter_mut().find(|(have, _)| *have == k) {
                        Some(slot) => slot.1 = v,
                        None => members.push((k, v)),
                    }
                }
                Json::Obj(members)
            }
            _ => Json::Null,
        })
    }
}
