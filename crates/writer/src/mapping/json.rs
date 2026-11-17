//! A mapping's JSON as a small tree, the way `XContentHelper.convertToMap`
//! hands a mapping to `MapperService`: objects keep their members in order,
//! numbers keep their text, duplicate keys are refused (Jackson's strict
//! duplicate detection, which OpenSearch turns on).
//!
//! A mapping is at most [`MAX_MAPPING_BYTES`] of text, so the tree is bounded
//! by its input: every node costs at most a constant plus the bytes it was
//! read from.

use std::fmt::Write as _;

use oss_rest::json::{JsonError, Limits, Token, TokenSink, Tokenizer};

use super::error::MappingError;

/// The most bytes a mapping's text may take: twice the descriptor's own
/// limit, as the reader allows.
pub const MAX_MAPPING_BYTES: usize = 8 << 20;

/// The deepest a mapping may nest: `index.mapping.depth.limit` is 20
/// objects, each two JSON levels (the object and its `properties`), plus
/// the root and a field's own parameters.
pub const MAX_DEPTH: usize = 64;

/// One JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as written.
    Num(String),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object, members in the order written.
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value as compact JSON, numbers as written.
    pub fn to_json(&self) -> String {
        // ALLOC: one string the size of the value's text.
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(text) => out.push_str(text),
            Json::Str(s) => write_str(out, s),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            Json::Obj(members) => {
                out.push('{');
                for (i, (k, v)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(out, k);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }

    /// The value as Java's `Object.toString` shows the value Jackson read:
    /// numbers as `Integer`, `Long` or `Double` print, maps and lists as
    /// `AbstractMap` and `AbstractCollection` print.
    pub fn java_string(&self) -> String {
        // ALLOC: one string the size of the value's text; parse and error paths.
        let mut out = String::new();
        self.write_java(&mut out);
        out
    }

    fn write_java(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(text) => out.push_str(&java_number(text)),
            Json::Str(s) => out.push_str(s),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write_java(out);
                }
                out.push(']');
            }
            Json::Obj(members) => {
                out.push('{');
                for (i, (k, v)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(k);
                    out.push('=');
                    v.write_java(out);
                }
                out.push('}');
            }
        }
    }
}

/// Whether a JSON number's text is an integer, as Jackson reads one: no
/// fraction and no exponent.
pub fn is_integral(text: &str) -> bool {
    !text.contains(['.', 'e', 'E'])
}

/// A JSON number's text as Java prints the value Jackson reads from it: an
/// integer as written (`Integer`, `Long` or `BigInteger`, all printed in
/// full), anything else as `Double.toString` prints it.
pub fn java_number(text: &str) -> String {
    if is_integral(text) {
        // ALLOC: the number's text; parse and error paths.
        return text.trim_start_matches('+').to_owned();
    }
    java_double(text.parse::<f64>().unwrap_or(f64::NAN))
}

/// `Double.toString` (JDK 19 and later: the shortest decimal that reads back
/// to the same double).
pub fn java_double(v: f64) -> String {
    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
    java_floating(v, format!("{v:e}"))
}

/// `Float.toString` (JDK 19 and later).
pub fn java_float(v: f32) -> String {
    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
    java_floating(f64::from(v), format!("{v:e}"))
}

/// Java's layout of a shortest decimal, given Rust's shortest `{:e}` form of
/// the same value: plain between 10^-3 and 10^7, computerized scientific
/// notation (`1.0E10`) outside it, always with a fractional part.
fn java_floating(v: f64, sci: String) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // `sci` is `[-]d[.ddd]e[-]x`.
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    // ALLOC: at most 17 digits.
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let abs = v.abs();
    // ALLOC: a few dozen bytes.
    let mut out = String::from(sign);
    if (1e-3..1e7).contains(&abs) {
        if exp >= 0 {
            let int_len = usize::try_from(exp).unwrap_or(0).saturating_add(1);
            if digits.len() <= int_len {
                out.push_str(&digits);
                for _ in digits.len()..int_len {
                    out.push('0');
                }
                out.push_str(".0");
            } else {
                out.push_str(&digits[..int_len]);
                out.push('.');
                out.push_str(&digits[int_len..]);
            }
        } else {
            out.push_str("0.");
            for _ in 1..exp.unsigned_abs() {
                out.push('0');
            }
            out.push_str(&digits);
        }
    } else {
        out.push_str(&digits[..1]);
        out.push('.');
        if digits.len() > 1 {
            out.push_str(&digits[1..]);
        } else {
            out.push('0');
        }
        out.push('E');
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        out.push_str(&exp.to_string());
    }
    out
}

/// Reads a whole JSON document of at most [`MAX_MAPPING_BYTES`].
///
/// # Errors
///
/// A [`MappingError`] of type `parse_exception` for malformed JSON, a
/// duplicate key or text past the limits.
pub fn parse(text: &[u8]) -> Result<Json, MappingError> {
    if text.len() > MAX_MAPPING_BYTES {
        return Err(MappingError::parse("the mapping is larger than its limit"));
    }
    let limits = Limits {
        max_depth: MAX_DEPTH,
        ..Limits::default()
    };
    let mut tokenizer = Tokenizer::new(limits);
    let mut tree = Builder {
        // ALLOC: one frame a nesting level, at most MAX_DEPTH.
        stack: Vec::new(),
        key: None,
        piece: None,
        root: None,
    };
    tokenizer.feed(text, &mut tree)?;
    tokenizer.finish(&mut tree)?;
    tree.root
        .ok_or_else(|| MappingError::parse("the mapping is empty"))
}

impl From<JsonError> for MappingError {
    fn from(e: JsonError) -> Self {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        MappingError::parse(&format!("failed to parse the mapping: {e}"))
    }
}

/// Builds the tree from tokens.
struct Builder {
    stack: Vec<(Option<String>, Json)>,
    key: Option<String>,
    piece: Option<String>,
    root: Option<Json>,
}

impl Builder {
    fn value(&mut self, value: Json) -> Result<(), MappingError> {
        let key = self.key.take();
        match self.stack.last_mut() {
            None => self.root = Some(value),
            Some((_, Json::Arr(items))) => items.push(value),
            Some((_, Json::Obj(members))) => {
                let key = key.ok_or_else(|| MappingError::parse("a value without a key"))?;
                if members.iter().any(|(k, _)| *k == key) {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::parse(&format!("Duplicate field '{key}'")));
                }
                members.push((key, value));
            }
            Some(_) => return Err(MappingError::parse("a value inside a scalar")),
        }
        Ok(())
    }
}

impl TokenSink for Builder {
    type Error = MappingError;

    fn token(&mut self, token: Token<'_>) -> Result<(), MappingError> {
        match token {
            Token::StartObject => {
                let key = self.key.take();
                // ALLOC: the members of one object, bounded by the mapping's bytes.
                self.stack.push((key, Json::Obj(Vec::new())));
            }
            Token::StartArray => {
                let key = self.key.take();
                // ALLOC: the items of one array, bounded by the mapping's bytes.
                self.stack.push((key, Json::Arr(Vec::new())));
            }
            Token::EndObject | Token::EndArray => {
                let (key, value) = self
                    .stack
                    .pop()
                    .ok_or_else(|| MappingError::parse("an unbalanced close"))?;
                self.key = key;
                self.value(value)?;
            }
            // ALLOC: one key, bounded by the mapping's bytes.
            Token::Key(k) => self.key = Some(k.to_owned()),
            Token::Str { text, last } => {
                // ALLOC: one string, bounded by the mapping's bytes.
                let piece = self.piece.get_or_insert_with(String::new);
                piece.push_str(text);
                if last {
                    let s = self.piece.take().unwrap_or_default();
                    self.value(Json::Str(s))?;
                }
            }
            // ALLOC: one number's text, at most the tokenizer's number limit.
            Token::Number(n) => self.value(Json::Num(n.to_owned()))?,
            Token::Bool(b) => self.value(Json::Bool(b))?,
            Token::Null => self.value(Json::Null)?,
        }
        Ok(())
    }
}

/// Writes a JSON string the way Jackson's generator escapes it: `"`, `\\`
/// and the control characters below U+0020, nothing else.
pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Java's `String.compareTo`: UTF-16 code unit order.
pub fn java_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_doubles() {
        for (v, want) in [
            (1.5, "1.5"),
            (2.0, "2.0"),
            (1000.0, "1000.0"),
            (1e7, "1.0E7"),
            (1.25e10, "1.25E10"),
            (0.001, "0.001"),
            (0.0001, "1.0E-4"),
            (0.0123, "0.0123"),
            (-3.5, "-3.5"),
            (123456.789, "123456.789"),
            (0.1, "0.1"),
        ] {
            assert_eq!(java_double(v), want, "{v}");
        }
        assert_eq!(java_float(2.0), "2.0");
        assert_eq!(java_float(0.1), "0.1");
        assert_eq!(java_number("12"), "12");
        assert_eq!(java_number("1e3"), "1000.0");
    }

    #[test]
    fn tree_keeps_order_and_refuses_duplicates() {
        let j = parse(br#"{"b":1,"a":[true,null,"x"],"c":{"d":1.5}}"#).unwrap();
        let Json::Obj(m) = &j else { panic!() };
        assert_eq!(
            m.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        assert_eq!(j.get("a").unwrap().java_string(), "[true, null, x]");
        assert_eq!(j.get("c").unwrap().java_string(), "{d=1.5}");
        let e = parse(br#"{"a":1,"a":2}"#).unwrap_err();
        assert!(e.reason.contains("Duplicate field 'a'"), "{e:?}");
        assert!(parse(b"{").is_err());
        assert!(parse(b"").is_err());
    }
}
