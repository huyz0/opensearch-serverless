//! What the point-in-time routes read off a request: a time value and the
//! ids of a delete body.

use oss_rest::json::{Limits, Token, TokenSink, Tokenizer};
use oss_rest::{Body, RestError};

/// The most ids one delete names.
pub(super) const MAX_IDS: usize = 1024;

/// The longest id a delete body names; a longer string is no id.
const MAX_ID_BYTES: usize = 256;

/// A time value as `TimeValue.parseTimeValue` reads it for the units a
/// keep-alive uses: whole numbers of `nanos`, `micros`, `ms`, `s`, `m`, `h`
/// or `d`, and a bare `0`. Milliseconds, rounded down.
pub(super) fn time_millis(name: &str, value: &str) -> Result<u64, RestError> {
    let bad = |why: &str| {
        // ALLOC: error path; the message names the value.
        RestError::illegal_argument(format!(
            "failed to parse setting [{name}] with value [{value}] as a time value: {why}"
        ))
    };
    if value == "0" {
        return Ok(0);
    }
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| bad("unit is missing or unrecognized"))?;
    let (digits, unit) = value.split_at(split);
    let number: u64 = digits
        .parse()
        .map_err(|_| bad("not a whole number of units"))?;
    let (times, per) = match unit {
        "nanos" => (1, 1_000_000),
        "micros" => (1, 1_000),
        "ms" => (1, 1),
        "s" => (1_000, 1),
        "m" => (60_000, 1),
        "h" => (3_600_000, 1),
        "d" => (86_400_000, 1),
        _ => return Err(bad("unit is missing or unrecognized")),
    };
    number
        .checked_mul(times)
        .and_then(|n| n.checked_div(per))
        .ok_or_else(|| bad("the value is too large"))
}

enum At {
    Start,
    Members,
    Value,
    Array,
    End,
}

/// Collects the `pit_id` of `{"pit_id": "x"}` or `{"pit_id": ["x", "y"]}`.
struct Ids {
    at: At,
    ids: Option<Vec<String>>,
    current: String,
}

fn refuse(reason: &str) -> RestError {
    RestError::illegal_argument(reason)
}

impl Ids {
    fn push(&mut self, text: &str, last: bool) -> Result<(), RestError> {
        if self.current.len().saturating_add(text.len()) > MAX_ID_BYTES {
            return Err(refuse("a pit_id is longer than any point in time id"));
        }
        self.current.push_str(text);
        if last {
            let ids = self.ids.get_or_insert_with(Vec::new);
            if ids.len() == MAX_IDS {
                return Err(refuse("a delete names too many point in time ids"));
            }
            ids.push(std::mem::take(&mut self.current));
        }
        Ok(())
    }
}

impl TokenSink for Ids {
    type Error = RestError;

    fn token(&mut self, token: Token<'_>) -> Result<(), RestError> {
        match (&self.at, token) {
            (At::Start, Token::StartObject) => self.at = At::Members,
            (At::Members, Token::Key("pit_id")) => {
                if self.ids.is_some() {
                    return Err(refuse("pit_id is given twice"));
                }
                self.ids = Some(Vec::new());
                self.at = At::Value;
            }
            (At::Members, Token::Key(other)) => {
                // ALLOC: error path; the field's name.
                return Err(refuse(&format!("unknown field [{other}]")));
            }
            (At::Members, Token::EndObject) => self.at = At::End,
            (At::Value, Token::StartArray) => self.at = At::Array,
            (At::Value, Token::Str { text, last }) => {
                self.push(text, last)?;
                if last {
                    self.at = At::Members;
                }
            }
            (At::Array, Token::Str { text, last }) => self.push(text, last)?,
            (At::Array, Token::EndArray) => self.at = At::Members,
            _ => return Err(refuse("pit_id is a string or an array of strings")),
        }
        Ok(())
    }
}

/// The ids a delete body names, read as it streams in.
///
/// # Errors
///
/// 400 for a body that is not JSON, is not an object of `pit_id` alone, holds
/// anything but strings there, or names none.
pub(super) fn ids_of(body: &mut Body<'_>) -> Result<Vec<String>, RestError> {
    let mut sink = Ids {
        at: At::Start,
        ids: None,
        // ALLOC: one id, at most MAX_ID_BYTES.
        current: String::new(),
    };
    let mut tokenizer = Tokenizer::new(Limits {
        max_depth: 4,
        max_key_bytes: 64,
        max_string_bytes: MAX_ID_BYTES as u64,
        max_number_bytes: 32,
        string_chunk: 64,
    });
    while let Some(chunk) = body.next_chunk()? {
        tokenizer.feed(chunk, &mut sink)?;
    }
    tokenizer.finish(&mut sink)?;
    match sink.ids {
        Some(ids) if !ids.is_empty() => Ok(ids),
        _ => Err(refuse(
            "name the views to release in a pit_id array, or delete _all",
        )),
    }
}
