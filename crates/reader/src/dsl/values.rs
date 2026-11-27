//! Reading a value token the way `AbstractXContentParser` reads it:
//! `intValue`, `floatValue` and `booleanValue` with coercion from strings,
//! `objectBytes` for a query's value, and Lucene's `LevenshteinDistance` for
//! OpenSearch's "did you mean" suggestions.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};

/// A query's value as `maybeConvertToBytesRef(parser.objectBytes())` keeps
/// it: a string, a number as written, or a boolean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A string.
    Str(String),
    /// A number, as written in the body.
    Number(String),
    /// A boolean.
    Bool(bool),
}

impl Value {
    /// The value as text: what `Object.toString()` gives for it, which
    /// field types parse.
    pub fn text(&self) -> &str {
        match self {
            Value::Str(s) | Value::Number(s) => s,
            Value::Bool(true) => "true",
            Value::Bool(false) => "false",
        }
    }
}

fn number_format(text: &str) -> DslError {
    // ALLOC: error path; one message.
    let reason = format!("For input string: \"{text}\"");
    DslError::Failure(crate::failure::Failure::plain(
        "number_format_exception",
        reason,
    ))
}

/// `parser.objectBytes()` of a value token, `None` for null.
pub fn object_value(p: &Parser<'_>) -> Result<Option<Value>, DslError> {
    Ok(match p.current_token() {
        // ALLOC: one value a query clause, bounded by the body's recording.
        Some(Kind::String) => Some(Value::Str(p.text().to_owned())),
        // ALLOC: one value a query clause, bounded by the body's recording.
        Some(Kind::Number) => Some(Value::Number(p.text().to_owned())),
        Some(Kind::Bool) => Some(Value::Bool(p.text() == "true")),
        Some(Kind::Null) => None,
        other => {
            return Err(DslError::parsing(
                p.location(),
                // ALLOC: error path; one message.
                format!(
                    "expected a value but found [{}]",
                    other.map_or("null", Kind::name)
                ),
            ));
        }
    })
}

/// `parser.intValue()`: a number (a fraction truncated, as Jackson
/// coerces) or a string holding one.
pub fn int_value(p: &Parser<'_>) -> Result<i32, DslError> {
    match p.current_token() {
        Some(Kind::Number | Kind::String) => parse_int(p.text()),
        _ => Err(number_format(p.text())),
    }
}

/// Whether `text` is a time value `TimeValue.parseTimeValue` reads: a number
/// (a fraction allowed) with a unit of `nanos`, `micros`, `ms`, `s`, `m`, `h`
/// or `d`, or the unitless `-1` and `0` it takes as they are. Any other text
/// is one this node does not read, so none is called valid that Java refuses.
pub fn is_time_value(text: &str) -> bool {
    let normalized = text.to_lowercase();
    let normalized = normalized.trim();
    let number = ["nanos", "micros", "ms", "s", "m", "h", "d"]
        .iter()
        .find_map(|unit| normalized.strip_suffix(unit));
    match number.map(str::trim) {
        Some(n) => {
            let (whole, fraction) = n.split_once('.').map_or((n, None), |(w, f)| (w, Some(f)));
            let digits = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
            digits(whole) && fraction.is_none_or(digits)
        }
        None => {
            let zeros = |d: &str| !d.is_empty() && d.bytes().all(|b| b == b'0');
            zeros(normalized)
                || normalized.strip_prefix('-').is_some_and(|d| {
                    d.strip_suffix('1')
                        .is_some_and(|z| z.bytes().all(|b| b == b'0'))
                })
        }
    }
}

/// Java's `Integer.parseInt`, with a decimal's fraction dropped as
/// `Number.intValue()` drops it.
pub fn parse_int(text: &str) -> Result<i32, DslError> {
    if let Ok(n) = text.parse::<i32>() {
        return Ok(n);
    }
    match text.parse::<f64>() {
        // ARITH: none; `as` saturates at the i32 range, as Java's (int) cast does.
        Ok(f) if f.is_finite() && text.contains(['.', 'e', 'E']) => Ok(f as i32),
        _ => Err(number_format(text)),
    }
}

/// `parser.floatValue()`: a number or a string holding one.
pub fn float_value(p: &Parser<'_>) -> Result<f32, DslError> {
    match p.current_token() {
        Some(Kind::Number | Kind::String) => p
            .text()
            .trim()
            .parse::<f32>()
            .map_err(|_| number_format(p.text())),
        _ => Err(number_format(p.text())),
    }
}

/// `parser.booleanValue()`: `true`/`false`, or a string of either.
pub fn bool_value(p: &Parser<'_>) -> Result<bool, DslError> {
    match (p.current_token(), p.text()) {
        (Some(Kind::Bool | Kind::String), "true") => Ok(true),
        (Some(Kind::Bool | Kind::String), "false") => Ok(false),
        (_, text) => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("Failed to parse value [{text}] as only [true] or [false] are allowed."),
        )),
    }
}

/// `parser.textOrNull()`: a value's text, `None` for null.
pub fn text_or_null<'r>(p: &Parser<'r>) -> Option<&'r str> {
    (p.current_token() != Some(Kind::Null)).then(|| p.text())
}

/// Lucene's `LevenshteinDistance.getDistance`: one minus the edit distance
/// over the longer length, in UTF-16 units.
pub fn levenshtein(target: &str, other: &str) -> f32 {
    // ALLOC: error path; two names of a misspelt field, short.
    let sa: Vec<u16> = target.encode_utf16().collect();
    // ALLOC: error path; as above.
    let ta: Vec<u16> = other.encode_utf16().collect();
    let (n, m) = (sa.len(), ta.len());
    if n == 0 || m == 0 {
        return if n == m { 1.0 } else { 0.0 };
    }
    // ALLOC: error path; one row per name length.
    let mut p: Vec<usize> = (0..=n).collect();
    // ALLOC: error path; as above.
    let mut d = vec![0usize; n.saturating_add(1)];
    for (j, &t_j) in ta.iter().enumerate() {
        d[0] = j.saturating_add(1);
        for i in 1..=n {
            let cost = usize::from(sa[i.saturating_sub(1)] != t_j);
            d[i] = d[i.saturating_sub(1)]
                .saturating_add(1)
                .min(p[i].saturating_add(1))
                .min(p[i.saturating_sub(1)].saturating_add(cost));
        }
        std::mem::swap(&mut p, &mut d);
    }
    // ARITH: none; float division of lengths that are not zero.
    1.0 - (p[n] as f32 / n.max(m) as f32)
}

/// `SuggestingErrorOnUnknown.suggest`: ` did you mean [x]?`, ` did you mean
/// any of [x, y]?`, or nothing, over the candidates closer than half.
pub fn suggest(unknown: &str, candidates: &[&str]) -> String {
    let mut scored: Vec<(f32, &str)> = candidates
        .iter()
        .map(|c| (levenshtein(unknown, c), *c))
        .filter(|(d, _)| *d > 0.5)
        // ALLOC: error path; at most one entry a candidate name.
        .collect();
    if scored.is_empty() {
        // ALLOC: error path; empty.
        return String::new();
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
    if let [(_, only)] = scored.as_slice() {
        // ALLOC: error path; one message.
        return format!(" did you mean [{only}]?");
    }
    // ALLOC: error path; the names joined.
    let keys: Vec<&str> = scored.iter().map(|(_, k)| *k).collect();
    // ALLOC: error path; one message.
    format!(" did you mean any of [{}]?", keys.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::tokens::Recorder;
    use oss_rest::json::{Limits, Tokenizer};

    fn at(doc: &str, f: impl FnOnce(&Parser<'_>)) {
        let mut rec = Recorder::new(1 << 16);
        let mut t = Tokenizer::new(Limits::default());
        t.feed(doc.as_bytes(), &mut rec).expect("feeds");
        t.finish(&mut rec).expect("finishes");
        let mut p = rec.parser();
        p.next_token();
        f(&p);
    }

    #[test]
    fn values_coerce_as_jackson_does() {
        at("7", |p| assert_eq!(int_value(p).ok(), Some(7)));
        at("\"8\"", |p| assert_eq!(int_value(p).ok(), Some(8)));
        at("2.9", |p| assert_eq!(int_value(p).ok(), Some(2)));
        at("\"x\"", |p| assert!(int_value(p).is_err()));
        at("true", |p| assert!(int_value(p).is_err()));
        at("1.5", |p| assert_eq!(float_value(p).ok(), Some(1.5)));
        at("\"2\"", |p| assert_eq!(float_value(p).ok(), Some(2.0)));
        at("null", |p| assert!(float_value(p).is_err()));
        at("\"z\"", |p| assert!(float_value(p).is_err()));
        at("true", |p| assert_eq!(bool_value(p).ok(), Some(true)));
        at("\"false\"", |p| assert_eq!(bool_value(p).ok(), Some(false)));
        at("1", |p| assert!(bool_value(p).is_err()));
        at("null", |p| assert_eq!(text_or_null(p), None));
        at("3", |p| assert_eq!(text_or_null(p), Some("3")));
        at("\"s\"", |p| {
            assert_eq!(object_value(p).ok().flatten(), Some(Value::Str("s".into())));
        });
        at("12", |p| {
            assert_eq!(
                object_value(p).ok().flatten(),
                Some(Value::Number("12".into()))
            );
        });
        at("false", |p| {
            assert_eq!(object_value(p).ok().flatten(), Some(Value::Bool(false)));
        });
        at("null", |p| assert_eq!(object_value(p).ok(), Some(None)));
        at("{}", |p| assert!(object_value(p).is_err()));
        assert_eq!(Value::Bool(true).text(), "true");
        assert_eq!(Value::Bool(false).text(), "false");
        assert_eq!(Value::Number("1".into()).text(), "1");
        assert!(parse_int("1e99999").is_err());
    }

    #[test]
    fn levenshtein_and_suggestions_match_lucene() {
        assert_eq!(levenshtein("", ""), 1.0);
        assert_eq!(levenshtein("a", ""), 0.0);
        assert_eq!(levenshtein("must", "must"), 1.0);
        assert_eq!(levenshtein("mst", "must"), 0.75);
        assert_eq!(
            suggest("mst", &["must", "should", "filter"]),
            " did you mean [must]?"
        );
        assert_eq!(
            suggest("matc", &["match", "match_all", "term"]),
            " did you mean [match]?"
        );
        assert_eq!(
            suggest("terms_", &["term", "terms", "terms_set"]),
            " did you mean any of [terms, term, terms_set]?"
        );
        assert_eq!(suggest("zzzz", &["must"]), "");
    }
}
