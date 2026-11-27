//! A search body's tokens, recorded once from the streaming tokenizer and
//! then read back by a pull parser shaped like `XContentParser`.
//!
//! OpenSearch's query parsers pull tokens (`nextToken`, `currentName`,
//! `text`, `getTokenLocation`), while `oss_rest::json::Tokenizer` pushes
//! them as body chunks arrive. [`Recorder`] is the tokenizer's sink: it keeps
//! each token's kind, location and text in two flat buffers, bounded by
//! [`Recorder::new`]'s limit, so no JSON tree and no copy of the body is
//! built. [`Parser`] then walks them as Jackson's parser would, and the
//! ported parsers read like their Java originals.

use oss_rest::json::{JsonError, Location, Token, TokenSink};

use crate::dsl::error::DslError;

/// A token's kind, `XContentParser.Token` without the value subtypes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `START_OBJECT`
    StartObject,
    /// `END_OBJECT`
    EndObject,
    /// `START_ARRAY`
    StartArray,
    /// `END_ARRAY`
    EndArray,
    /// `FIELD_NAME`
    FieldName,
    /// `VALUE_STRING`
    String,
    /// `VALUE_NUMBER`
    Number,
    /// `VALUE_BOOLEAN`
    Bool,
    /// `VALUE_NULL`
    Null,
}

impl Kind {
    /// `Token.isValue()`.
    pub fn is_value(self) -> bool {
        matches!(self, Kind::String | Kind::Number | Kind::Bool | Kind::Null)
    }

    /// The name Jackson's `JsonToken` prints, as OpenSearch's messages show it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::StartObject => "START_OBJECT",
            Kind::EndObject => "END_OBJECT",
            Kind::StartArray => "START_ARRAY",
            Kind::EndArray => "END_ARRAY",
            Kind::FieldName => "FIELD_NAME",
            Kind::String => "VALUE_STRING",
            Kind::Number => "VALUE_NUMBER",
            Kind::Bool => "VALUE_BOOLEAN",
            Kind::Null => "VALUE_NULL",
        }
    }
}

/// One recorded token: its kind, where it starts, and its text as a range
/// of the recorder's text buffer (a key, a string, a number, `true` or
/// `false`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tok {
    kind: Kind,
    line: u32,
    column: u32,
    start: u32,
    end: u32,
}

/// What one recorded token costs, for the limit.
const TOKEN_BYTES: usize = std::mem::size_of::<Tok>();

/// The tokenizer's sink: records every token, within a byte limit.
#[derive(Debug)]
pub struct Recorder {
    tokens: Vec<Tok>,
    text: String,
    /// Where the string being received in pieces starts.
    open_string: Option<u32>,
    limit: usize,
}

impl Recorder {
    /// A recorder that refuses a body whose tokens and text would take more
    /// than `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            // ALLOC: grows with the body, at most `limit` bytes (checked per token).
            tokens: Vec::new(),
            // ALLOC: grows with the body, at most `limit` bytes (checked per token).
            text: String::new(),
            open_string: None,
            limit,
        }
    }

    /// The bytes the recording holds now: what the caller charges to the
    /// request's memory reservation.
    pub fn bytes(&self) -> usize {
        self.tokens
            .capacity()
            .saturating_mul(TOKEN_BYTES)
            .saturating_add(self.text.capacity())
    }

    /// The most [`Self::bytes`] can reach once `input` more bytes of body
    /// are fed: what a caller reserves before feeding them. Each token takes
    /// at least one byte of input (and the last may end at the finish), and
    /// each byte adds at most one byte of text; a growing buffer at most
    /// doubles past what it needs, and nothing grows past the limit.
    pub fn bytes_after(&self, input: usize) -> usize {
        let grown = |len: usize, cap: usize, more: usize, most: usize| {
            let needed = len.saturating_add(more).min(most);
            if needed <= cap {
                cap
            } else {
                needed.saturating_mul(2).max(8)
            }
        };
        let most_tokens = (self.limit / TOKEN_BYTES).saturating_add(1);
        let tokens = grown(
            self.tokens.len(),
            self.tokens.capacity(),
            input.saturating_add(1),
            most_tokens,
        );
        let text = grown(self.text.len(), self.text.capacity(), input, self.limit);
        tokens.saturating_mul(TOKEN_BYTES).saturating_add(text)
    }

    /// Whether nothing was recorded (an empty or whitespace-only body).
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// A parser over what was recorded.
    pub fn parser(&self) -> Parser<'_> {
        Parser {
            rec: self,
            pos: None,
            name: std::cell::RefCell::new(None),
        }
    }

    fn used(&self) -> usize {
        self.tokens
            .len()
            .saturating_add(1)
            .saturating_mul(TOKEN_BYTES)
            .saturating_add(self.text.len())
    }

    fn check(&self, more: usize) -> Result<(), DslError> {
        if self.used().saturating_add(more) > self.limit {
            return Err(DslError::TooLarge { limit: self.limit });
        }
        Ok(())
    }

    fn text_end(&self) -> u32 {
        u32::try_from(self.text.len()).unwrap_or(u32::MAX)
    }

    fn push(&mut self, kind: Kind, at: Location, start: u32) {
        self.tokens.push(Tok {
            kind,
            line: u32::try_from(at.line).unwrap_or(u32::MAX),
            column: u32::try_from(at.column).unwrap_or(u32::MAX),
            start,
            end: self.text_end(),
        });
    }
}

impl From<JsonError> for DslError {
    fn from(e: JsonError) -> Self {
        DslError::Json(e)
    }
}

impl TokenSink for Recorder {
    type Error = DslError;

    fn token(&mut self, token: Token<'_>) -> Result<(), DslError> {
        self.token_at(token, Location::default())
    }

    fn token_at(&mut self, token: Token<'_>, at: Location) -> Result<(), DslError> {
        let text = match token {
            Token::Key(t) | Token::Number(t) => t,
            Token::Str { text, .. } => text,
            Token::Bool(true) => "true",
            Token::Bool(false) => "false",
            _ => "",
        };
        self.check(text.len())?;
        let start = match token {
            Token::Str { .. } => *self.open_string.get_or_insert(self.text_end()),
            _ => self.text_end(),
        };
        self.text.push_str(text);
        let kind = match token {
            Token::StartObject => Kind::StartObject,
            Token::EndObject => Kind::EndObject,
            Token::StartArray => Kind::StartArray,
            Token::EndArray => Kind::EndArray,
            Token::Key(_) => Kind::FieldName,
            Token::Str { last: false, .. } => return Ok(()),
            Token::Str { last: true, .. } => {
                self.open_string = None;
                Kind::String
            }
            Token::Number(_) => Kind::Number,
            Token::Bool(_) => Kind::Bool,
            Token::Null => Kind::Null,
        };
        self.push(kind, at, start);
        Ok(())
    }
}

/// A pull parser over a [`Recorder`]'s tokens, after `XContentParser`.
#[derive(Clone, Debug)]
pub struct Parser<'r> {
    rec: &'r Recorder,
    /// The current token; `None` before the first `next_token`.
    pos: Option<usize>,
    /// The `_name` the query just parsed carried, taken by `parse_inner`.
    name: std::cell::RefCell<Option<String>>,
}

impl<'r> Parser<'r> {
    /// Keeps the `_name` of the query being parsed for [`Parser::take_name`].
    pub fn set_name(&self, name: String) {
        *self.name.borrow_mut() = Some(name);
    }

    /// The `_name` the query just parsed carried, once.
    pub fn take_name(&self) -> Option<String> {
        self.name.borrow_mut().take()
    }

    fn tok(&self, at: usize) -> Option<&'r Tok> {
        self.rec.tokens.get(at)
    }

    fn current(&self) -> Option<&'r Tok> {
        self.pos.and_then(|p| self.tok(p))
    }

    /// `nextToken()`: advances, `None` past the end.
    pub fn next_token(&mut self) -> Option<Kind> {
        let next = self.pos.map_or(0, |p| p.saturating_add(1));
        self.pos = Some(next.min(self.rec.tokens.len()));
        self.current_token()
    }

    /// `currentToken()`.
    pub fn current_token(&self) -> Option<Kind> {
        self.current().map(|t| t.kind)
    }

    /// The current token's text: a key's name, a string's value, a number
    /// as written, `true` or `false`; empty for structure and null.
    pub fn text(&self) -> &'r str {
        self.current()
            .and_then(|t| {
                self.rec
                    .text
                    .get(usize::try_from(t.start).ok()?..usize::try_from(t.end).ok()?)
            })
            .unwrap_or("")
    }

    /// `currentName()`: on a field name its name; on a value, the name of
    /// the field it is the value of (`None` in an array or at the root).
    pub fn current_name(&self) -> Option<&'r str> {
        let pos = self.pos?;
        let kind = self.tok(pos)?.kind;
        if kind == Kind::FieldName {
            return Some(self.text());
        }
        if matches!(kind, Kind::EndObject | Kind::EndArray) {
            return None;
        }
        let prev = pos.checked_sub(1)?;
        let before = self.tok(prev)?;
        (before.kind == Kind::FieldName).then(|| {
            self.rec
                .text
                .get(before.start as usize..before.end as usize)
                .unwrap_or("")
        })
    }

    /// `getTokenLocation()`: where the current token starts.
    pub fn location(&self) -> Location {
        self.current()
            .map_or(Location { line: 1, column: 1 }, |t| Location {
                line: u64::from(t.line),
                column: u64::from(t.column),
            })
    }

    /// `skipChildren()`: on a start token, moves to its matching end; on
    /// anything else, stays.
    pub fn skip_children(&mut self) {
        if !matches!(
            self.current_token(),
            Some(Kind::StartObject | Kind::StartArray)
        ) {
            return;
        }
        let mut depth = 0usize;
        while let Some(kind) = self.current_token() {
            match kind {
                Kind::StartObject | Kind::StartArray => depth = depth.saturating_add(1),
                Kind::EndObject | Kind::EndArray => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
            self.next_token();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_rest::json::{Limits, Tokenizer};

    pub(crate) fn record(doc: &str) -> Recorder {
        let mut rec = Recorder::new(1 << 20);
        let mut t = Tokenizer::new(Limits {
            string_chunk: 4,
            ..Limits::default()
        });
        t.feed(doc.as_bytes(), &mut rec).expect("feeds");
        t.finish(&mut rec).expect("finishes");
        rec
    }

    #[test]
    fn tokens_read_back_with_names_text_and_locations() {
        let rec = record(r#"{"query":{"term":{"tag":"a long string value"}},"n":[1,true,null]}"#);
        let mut p = rec.parser();
        assert_eq!(p.current_token(), None);
        assert_eq!(p.current_name(), None);
        assert_eq!(p.next_token(), Some(Kind::StartObject));
        assert_eq!(p.current_name(), None);
        assert_eq!(p.next_token(), Some(Kind::FieldName));
        assert_eq!((p.text(), p.current_name()), ("query", Some("query")));
        assert_eq!(p.next_token(), Some(Kind::StartObject));
        assert_eq!(p.current_name(), Some("query"));
        p.next_token();
        p.next_token();
        p.next_token();
        assert_eq!(p.next_token(), Some(Kind::String));
        assert_eq!(p.text(), "a long string value", "pieces joined");
        assert_eq!(p.current_name(), Some("tag"));
        assert_eq!(
            p.location(),
            Location {
                line: 1,
                column: 25
            }
        );
        p.next_token();
        assert_eq!(p.current_name(), None, "an end token has no name");
        p.next_token();
        p.next_token();
        assert_eq!(p.text(), "n");
        assert_eq!(p.next_token(), Some(Kind::StartArray));
        assert_eq!(p.next_token(), Some(Kind::Number));
        assert_eq!((p.text(), p.current_name()), ("1", None));
        assert_eq!(p.next_token(), Some(Kind::Bool));
        assert_eq!(p.text(), "true");
        assert_eq!(p.next_token(), Some(Kind::Null));
        assert_eq!(p.text(), "");
        p.next_token();
        p.next_token();
        assert_eq!(p.next_token(), None);
        assert_eq!(p.next_token(), None, "stays past the end");
        assert_eq!(p.location(), Location { line: 1, column: 1 });
    }

    #[test]
    fn skip_children_moves_to_the_matching_end() {
        let rec = record(r#"{"a":{"b":[{"c":{}}],"d":1},"e":2}"#);
        let mut p = rec.parser();
        p.next_token();
        p.next_token();
        p.next_token();
        p.skip_children();
        assert_eq!(p.current_token(), Some(Kind::EndObject));
        p.next_token();
        assert_eq!(p.text(), "e");
        p.skip_children();
        assert_eq!(p.text(), "e", "a field name has no children");
    }

    #[test]
    fn kinds_name_themselves_as_jackson_does() {
        let all = [
            (Kind::StartObject, "START_OBJECT", false),
            (Kind::EndObject, "END_OBJECT", false),
            (Kind::StartArray, "START_ARRAY", false),
            (Kind::EndArray, "END_ARRAY", false),
            (Kind::FieldName, "FIELD_NAME", false),
            (Kind::String, "VALUE_STRING", true),
            (Kind::Number, "VALUE_NUMBER", true),
            (Kind::Bool, "VALUE_BOOLEAN", true),
            (Kind::Null, "VALUE_NULL", true),
        ];
        for (kind, name, value) in all {
            assert_eq!((kind.name(), kind.is_value()), (name, value));
        }
    }

    #[test]
    fn what_a_chunk_can_grow_the_recording_to_is_known_before_it_is_fed() {
        let body =
            br#"{"query":{"bool":{"should":[{"term":{"t":"a"}},{"term":{"t":"bb"}}]}},"size":10}"#;
        for split in 1..body.len() {
            let mut rec = Recorder::new(1 << 16);
            let mut t = Tokenizer::new(Limits::default());
            for chunk in body.chunks(split) {
                let bound = rec.bytes_after(chunk.len());
                t.feed(chunk, &mut rec).expect("feeds");
                assert!(rec.bytes() <= bound, "{} > {bound} at {split}", rec.bytes());
            }
            let bound = rec.bytes_after(0);
            t.finish(&mut rec).expect("finishes");
            assert!(rec.bytes() <= bound);
        }
        let small = Recorder::new(64);
        assert!(small.bytes_after(1 << 20) <= 2 * (64 / TOKEN_BYTES + 1) * TOKEN_BYTES + 128);
    }

    #[test]
    fn the_limit_holds_and_refuses_one_more_token() {
        let doc = r#"{"abc":1}"#;
        let full = record(doc).used();
        let mut at = Recorder::new(full.saturating_sub(TOKEN_BYTES));
        let mut t = Tokenizer::new(Limits::default());
        t.feed(doc.as_bytes(), &mut at).expect("fits exactly");
        let mut over = Recorder::new(full.saturating_sub(TOKEN_BYTES).saturating_sub(1));
        let mut t = Tokenizer::new(Limits::default());
        assert!(matches!(
            t.feed(doc.as_bytes(), &mut over),
            Err(DslError::TooLarge { .. })
        ));
        assert!(record(" ").is_empty());
        assert!(record("{}").bytes() >= 2 * TOKEN_BYTES);
        let mut plain = Recorder::new(100);
        plain.token(Token::Null).expect("a token without location");
        assert_eq!(plain.parser().location(), Location { line: 1, column: 1 });
    }
}
