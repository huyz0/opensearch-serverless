//! The tokenizer: `chunkjson`'s push tokenizer, with this module's limits on
//! keys and strings, its pieces, and its errors.

use super::{Completion, ErrorKind, JsonError, Limits, Location, Token, TokenSink};
use chunkjson::{Config, Event, Framing, Position, Sink};

/// A streaming JSON tokenizer for one document; see the module docs.
#[derive(Debug)]
pub struct Tokenizer {
    limits: Limits,
    inner: chunkjson::Tokenizer,
    /// The key being read, whole keys being handed on.
    key: String,
    /// Where the key being read starts.
    key_at: Location,
    /// A string piece gathered across fragments.
    piece: String,
    /// Bytes of the string value being read so far.
    string_len: u64,
    /// Bytes fed since `new` or `reset`.
    offset: u64,
    /// A complete root value was read.
    root_seen: bool,
    failed: Option<JsonError>,
}

/// Why a feed stopped: the tokenizer's error, a limit of this module, or the
/// sink's own error.
enum Stop<E> {
    Inner(chunkjson::Error),
    Limit(JsonError),
    Sink(E),
}

impl<E> From<chunkjson::Error> for Stop<E> {
    fn from(e: chunkjson::Error) -> Self {
        Self::Inner(e)
    }
}

/// Hands `chunkjson`'s events on to a [`TokenSink`] as tokens.
struct Adapter<'t, 's, S> {
    limits: Limits,
    key: &'t mut String,
    key_at: &'t mut Location,
    piece: &'t mut String,
    string_len: &'t mut u64,
    root_seen: &'t mut bool,
    sink: &'s mut S,
}

impl<S: TokenSink> Adapter<'_, '_, S> {
    fn hand(&mut self, token: Token<'_>, at: Location) -> Result<(), Stop<S::Error>> {
        self.sink.token_at(token, at).map_err(Stop::Sink)
    }

    /// A string fragment, handed on in pieces of `string_chunk` bytes (the
    /// last one shorter), each ending on a character boundary. A piece that
    /// spans fragments is gathered in `piece`; one inside a fragment is
    /// handed on as it lies.
    fn string(
        &mut self,
        mut text: &str,
        last: bool,
        at: Location,
        start: Position,
    ) -> Result<(), Stop<S::Error>> {
        let len = u64::try_from(text.len()).unwrap_or(u64::MAX);
        *self.string_len = self.string_len.saturating_add(len);
        if *self.string_len > self.limits.max_string_bytes {
            return Err(Stop::Limit(limit(ErrorKind::StringTooLong, start)));
        }
        let size = self.limits.string_chunk;
        // each pass hands on a piece or gathers the rest, so it is bounded
        for _ in 0..=text.len() {
            let room = size.saturating_sub(self.piece.len());
            if text.len() < room {
                // ALLOC: none; `piece` holds `string_chunk` from `new`.
                self.piece.push_str(text);
                break;
            }
            let (head, rest) = text.split_at(text.floor_char_boundary(room));
            text = rest;
            let piece_last = last && text.is_empty();
            if self.piece.is_empty() {
                self.hand(
                    Token::Str {
                        text: head,
                        last: piece_last,
                    },
                    at,
                )?;
            } else {
                self.piece.push_str(head);
                let token = Token::Str {
                    text: self.piece,
                    last: piece_last,
                };
                self.sink.token_at(token, at).map_err(Stop::Sink)?;
                self.piece.clear();
            }
            if piece_last {
                return Ok(());
            }
        }
        if last {
            let token = Token::Str {
                text: self.piece,
                last: true,
            };
            self.sink.token_at(token, at).map_err(Stop::Sink)?;
            self.piece.clear();
        }
        Ok(())
    }
}

impl<S: TokenSink> Sink for Adapter<'_, '_, S> {
    type Error = Stop<S::Error>;

    /// Only reached if the tokenizer ever drops a position: tokens then
    /// read as starting at line 0.
    fn event(&mut self, event: Event<'_>) -> Result<(), Self::Error> {
        self.event_at(event, Position::default())
    }

    fn event_at(&mut self, event: Event<'_>, start: Position) -> Result<(), Self::Error> {
        let at = Location {
            line: start.line,
            column: start.column,
        };
        match event {
            Event::BeginObject => self.hand(Token::StartObject, at),
            Event::EndObject => self.hand(Token::EndObject, at),
            Event::BeginArray => self.hand(Token::StartArray, at),
            Event::EndArray => self.hand(Token::EndArray, at),
            Event::Key(t) => {
                if t.first {
                    self.key.clear();
                    *self.key_at = at;
                }
                if self.key.len().saturating_add(t.fragment.len()) > self.limits.max_key_bytes {
                    return Err(Stop::Limit(limit(ErrorKind::KeyTooLong, start)));
                }
                // ALLOC: none; `key` holds `max_key_bytes` from `new`.
                self.key.push_str(t.fragment);
                if !t.last {
                    return Ok(());
                }
                let key_at = *self.key_at;
                self.sink
                    .token_at(Token::Key(self.key), key_at)
                    .map_err(Stop::Sink)
            }
            Event::String(t) => {
                if t.first {
                    *self.string_len = 0;
                    self.piece.clear();
                }
                self.string(t.fragment, t.last, at, start)
            }
            Event::Number(n) => self.hand(Token::Number(n), at),
            Event::Bool(b) => self.hand(Token::Bool(b), at),
            Event::Null => self.hand(Token::Null, at),
            Event::EndDocument { .. } => {
                *self.root_seen = true;
                Ok(())
            }
            Event::NeedMore | Event::Eof => Ok(()),
        }
    }
}

/// A limit error at the start of the token that broke it.
fn limit(kind: ErrorKind, start: Position) -> JsonError {
    JsonError {
        kind,
        offset: start.offset,
        line: start.line,
        column: start.column,
    }
}

impl Tokenizer {
    /// A tokenizer with `limits`, allocating its buffers once.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            inner: chunkjson::Tokenizer::new(config(&limits)),
            // ALLOC: once per tokenizer; the longest key.
            key: String::with_capacity(limits.max_key_bytes),
            key_at: Location::default(),
            // ALLOC: once per tokenizer; one string piece.
            piece: String::with_capacity(limits.string_chunk),
            string_len: 0,
            offset: 0,
            root_seen: false,
            failed: bad_limits(&limits),
        }
    }

    /// Readies the tokenizer for a new document, keeping its allocations.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.key.clear();
        self.key_at = Location::default();
        self.piece.clear();
        self.string_len = 0;
        self.offset = 0;
        self.root_seen = false;
        self.failed = bad_limits(&self.limits);
    }

    /// Bytes fed so far.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Feeds the next chunk, handing complete tokens to `sink`. After an
    /// error from `sink` the rest of the chunk is dropped, so the tokenizer
    /// must be [`reset`](Self::reset) before it is fed again.
    pub fn feed<S: TokenSink>(&mut self, chunk: &[u8], sink: &mut S) -> Result<(), S::Error> {
        let result = self.run(chunk, false, sink);
        self.offset = self
            .offset
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        result
    }

    /// Ends the document: `Empty` for whitespace and comments only, `Value`
    /// for one complete value, an error for anything unfinished.
    pub fn finish<S: TokenSink>(&mut self, sink: &mut S) -> Result<Completion, S::Error> {
        self.run(&[], true, sink)?;
        Ok(if self.root_seen {
            Completion::Value
        } else {
            Completion::Empty
        })
    }

    fn run<S: TokenSink>(&mut self, chunk: &[u8], eof: bool, sink: &mut S) -> Result<(), S::Error> {
        if let Some(e) = self.failed {
            return Err(e.into());
        }
        let mut adapter = Adapter {
            limits: self.limits,
            key: &mut self.key,
            key_at: &mut self.key_at,
            piece: &mut self.piece,
            string_len: &mut self.string_len,
            root_seen: &mut self.root_seen,
            sink,
        };
        let stop = match self.inner.feed(chunk, eof, &mut adapter) {
            Ok(_) => return Ok(()),
            Err(stop) => stop,
        };
        let e = match stop {
            Stop::Sink(e) => return Err(e),
            Stop::Limit(e) => e,
            Stop::Inner(e) => self.convert(e, chunk),
        };
        self.failed = Some(e);
        Err(e.into())
    }

    /// `chunkjson`'s error as this module's, naming the offending byte where
    /// the kind carries one; the byte is in `chunk`, which the tokenizer was
    /// reading when it failed.
    fn convert(&self, e: chunkjson::Error, chunk: &[u8]) -> JsonError {
        let byte = || {
            let i = e.offset().checked_sub(self.offset)?;
            chunk.get(usize::try_from(i).ok()?).copied()
        };
        use chunkjson::ErrorKind as K;
        let kind = match e.kind() {
            K::UnexpectedByte(b) => ErrorKind::Unexpected(b),
            K::InvalidNumber => ErrorKind::BadNumber,
            K::NumberTooLong => ErrorKind::NumberTooLong,
            K::InvalidEscape => ErrorKind::BadEscape,
            K::LoneSurrogate => ErrorKind::LoneSurrogate,
            K::InvalidUtf8 => ErrorKind::InvalidUtf8,
            K::ControlCharacter => ErrorKind::ControlInString(byte().unwrap_or(0)),
            K::DepthExceeded => ErrorKind::TooDeep,
            K::TrailingData => ErrorKind::Trailing(byte().unwrap_or(0)),
            K::UnexpectedEof => ErrorKind::UnexpectedEnd,
            // single framing never refuses a line break, so only a kind
            // added later reaches here: the byte it stopped at
            _ => ErrorKind::Unexpected(byte().unwrap_or(0)),
        };
        JsonError {
            kind,
            offset: e.offset(),
            line: e.line(),
            column: e.column(),
        }
    }
}

/// The `chunkjson` configuration for `limits`: one document, comments and
/// empty input allowed, as Jackson's factory allows them.
fn config(limits: &Limits) -> Config {
    Config {
        max_depth: limits.max_depth,
        max_number_len: limits.max_number_bytes,
        comments: true,
        allow_empty: true,
        ..Config::with_framing(Framing::Single)
    }
}

/// `BadLimits` when a string piece could not hold one character.
fn bad_limits(limits: &Limits) -> Option<JsonError> {
    (limits.string_chunk < 4).then_some(JsonError {
        kind: ErrorKind::BadLimits,
        offset: 0,
        line: 1,
        column: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::tests_sink::Seen;
    use super::*;

    fn limits(string_chunk: usize) -> Limits {
        Limits {
            string_chunk,
            ..Limits::default()
        }
    }

    #[test]
    fn a_full_piece_goes_out_in_the_feed_that_completes_it() {
        let mut t = Tokenizer::new(limits(4));
        let mut seen = Seen::default();
        t.feed(b"\"ab", &mut seen).expect("feeds");
        assert!(seen.0.is_empty(), "half a piece waits");
        t.feed(b"cd", &mut seen).expect("feeds");
        assert_eq!(seen.0, ["Str { text: \"abcd\", last: false }"]);
        t.feed(b"efgh", &mut seen).expect("feeds");
        assert_eq!(seen.0.len(), 2, "a piece within one feed is not held");
    }

    #[test]
    fn a_refused_byte_from_an_earlier_feed_is_still_named() {
        let mut t = Tokenizer::new(Limits::default());
        let mut seen = Seen::default();
        t.feed(b"{} /", &mut seen).expect("feeds");
        let e = t.feed(b"x", &mut seen).expect_err("not a comment");
        assert_eq!((e.kind, e.offset), (ErrorKind::Unexpected(b'/'), 3));
    }

    #[test]
    fn an_event_without_a_position_still_reaches_the_sink() {
        let (mut key, mut piece) = (String::new(), String::new());
        let (mut key_at, mut string_len, mut root_seen) = (Location::default(), 0, false);
        let mut seen = Seen::default();
        let mut adapter = Adapter {
            limits: Limits::default(),
            key: &mut key,
            key_at: &mut key_at,
            piece: &mut piece,
            string_len: &mut string_len,
            root_seen: &mut root_seen,
            sink: &mut seen,
        };
        assert!(adapter.event(Event::Null).is_ok());
        assert_eq!(seen.0, ["Null"]);
    }
}

/// Where this tokenizer differs from the hand-written one it replaced,
/// pinned: chunkjson ends a number where its grammar ends, as Jackson does,
/// and places limit and UTF-8 errors without regard to chunking.
#[cfg(test)]
mod pinned {
    use super::*;

    fn run(doc: &[u8], lim: Limits) -> (Vec<String>, Result<Completion, JsonError>) {
        let mut t = Tokenizer::new(lim);
        let mut seen = tests_sink::Seen::default();
        let r = t.feed(doc, &mut seen).and_then(|()| t.finish(&mut seen));
        (seen.0, r)
    }

    fn err(doc: &[u8], lim: Limits) -> (ErrorKind, u64, u64) {
        let e = run(doc, lim).1.expect_err("refused");
        (e.kind, e.offset, e.column)
    }

    #[test]
    fn a_number_ends_where_its_grammar_ends() {
        let d = Limits::default();
        let (tokens, r) = run(b"[1-2]", d);
        assert_eq!(tokens, ["StartArray", "Number(\"1\")"]);
        assert_eq!(
            r.map_err(|e| (e.kind, e.offset)),
            Err((ErrorKind::Unexpected(b'-'), 2))
        );
        assert_eq!(err(b"[1.5.3]", d), (ErrorKind::Unexpected(b'.'), 4, 5));
        assert_eq!(
            err(b"{\"a\":1e1.5}", d),
            (ErrorKind::Unexpected(b'.'), 8, 9)
        );
        assert_eq!(err(b"[01]", d), (ErrorKind::BadNumber, 2, 3));
        assert_eq!(err(b"01", d), (ErrorKind::BadNumber, 1, 2));
    }

    #[test]
    fn limits_and_bad_utf8_are_placed_whatever_the_chunks() {
        let key = Limits {
            max_key_bytes: 3,
            ..Limits::default()
        };
        assert_eq!(
            err("{\"é😀\":1}".as_bytes(), key),
            (ErrorKind::KeyTooLong, 1, 2)
        );
        let string = Limits {
            max_string_bytes: 2,
            ..Limits::default()
        };
        assert_eq!(err(b"[\"abc\"]", string), (ErrorKind::StringTooLong, 1, 2));
        let number = Limits {
            max_number_bytes: 6,
            ..Limits::default()
        };
        assert_eq!(err(b"12345678", number), (ErrorKind::NumberTooLong, 6, 7));
        assert_eq!(
            err(b"\"abc\xff\"", Limits::default()),
            (ErrorKind::InvalidUtf8, 4, 5)
        );
        // bad UTF-8 is refused as it is fed, before the missing end
        let mut t = Tokenizer::new(Limits::default());
        let e = t
            .feed(b"\"a\xff", &mut tests_sink::Seen::default())
            .expect_err("bad");
        assert_eq!(e.kind, ErrorKind::InvalidUtf8);
    }

    #[test]
    fn a_string_ending_on_a_piece_boundary_ends_with_that_piece() {
        let (tokens, _) = run(
            b"\"abcd\"",
            Limits {
                string_chunk: 4,
                ..Limits::default()
            },
        );
        assert_eq!(tokens, ["Str { text: \"abcd\", last: true }"]);
    }

    #[test]
    fn a_failed_feed_still_counts_its_bytes() {
        let mut t = Tokenizer::new(Limits::default());
        assert!(t.feed(b"]null", &mut tests_sink::Seen::default()).is_err());
        assert_eq!(t.offset(), 5);
    }
}

/// A sink both test modules share.
#[cfg(test)]
mod tests_sink {
    use super::*;

    /// Each token, as text.
    #[derive(Default)]
    pub(super) struct Seen(pub(super) Vec<String>);

    impl TokenSink for Seen {
        type Error = JsonError;
        fn token(&mut self, t: Token<'_>) -> Result<(), JsonError> {
            self.0.push(format!("{t:?}"));
            Ok(())
        }
    }
}
