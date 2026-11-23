//! The values a template reads: what OpenSearch's `XContentParser.map()`
//! gives for `params`, and how Java prints them.

/// A JSON value as the Java holds it: a `LinkedHashMap`, an `ArrayList`, a
/// `String`, an `Integer`, `Long` or `BigInteger`, a `Double`, a `Boolean` or
/// null.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    /// JSON null.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer that fits a `long`.
    Int(i64),
    /// An integer beyond a `long`: a `BigInteger`, as its digits.
    Big(String),
    /// A number with a fraction or an exponent: a `Double`.
    Double(f64),
    /// A string.
    Str(String),
    /// An array.
    List(Vec<Val>),
    /// An object, in the order its keys first appeared; a repeated key
    /// replaces the value and keeps its place, as `LinkedHashMap.put` does.
    Map(Vec<(String, Val)>),
}

impl Val {
    /// The value of `key` in an object.
    pub fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Adds `key` to an object as `LinkedHashMap.put` does.
    pub fn put(entries: &mut Vec<(String, Val)>, key: String, value: Val) {
        match entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            // ALLOC: one entry a key of the body, which the recording bounds.
            None => entries.push((key, value)),
        }
    }

    /// `Object.toString()` of the value, as the handler's `stringify` gives
    /// it for a value read from a scope: an array it holds is a
    /// `CollectionMap`, printed as a map of its indexes.
    pub fn stringify(&self) -> String {
        // ALLOC: the template, its params and its output, which the request limits bound.
        let mut out = String::new();
        self.write_top(&mut out);
        out
    }

    /// `toString()` of a value held inside a collection.
    pub fn element_string(&self) -> String {
        // ALLOC: the template, its params and its output, which the request limits bound.
        let mut out = String::new();
        self.write_java(&mut out);
        out
    }

    fn write_top(&self, out: &mut String) {
        match self {
            Val::List(items) => {
                out.push('{');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    // ALLOC: the template, its params and its output, which the request limits bound.
                    out.push_str(&i.to_string());
                    out.push('=');
                    item.write_java(out);
                }
                out.push('}');
            }
            other => other.write_java(out),
        }
    }

    /// `toString()` of a value held inside a collection.
    fn write_java(&self, out: &mut String) {
        match self {
            Val::Null => out.push_str("null"),
            Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            // ALLOC: the template, its params and its output, which the request limits bound.
            Val::Int(i) => out.push_str(&i.to_string()),
            Val::Big(digits) => out.push_str(digits),
            Val::Double(d) => out.push_str(&java_double(*d)),
            Val::Str(s) => out.push_str(s),
            Val::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write_java(out);
                }
                out.push(']');
            }
            Val::Map(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
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

    /// The value as the JSON generator writes it.
    pub fn write_json(&self, out: &mut String) {
        match self {
            Val::Null => out.push_str("null"),
            Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            // ALLOC: the template, its params and its output, which the request limits bound.
            Val::Int(i) => out.push_str(&i.to_string()),
            Val::Big(digits) => out.push_str(digits),
            Val::Double(d) => {
                if d.is_finite() {
                    out.push_str(&java_double(*d));
                } else {
                    out.push('"');
                    out.push_str(&java_double(*d));
                    out.push('"');
                }
            }
            Val::Str(s) => {
                out.push('"');
                json_escape(s, out);
                out.push('"');
            }
            Val::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            Val::Map(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    json_escape(k, out);
                    out.push_str("\":");
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

/// Jackson's `JsonStringEncoder.quoteAsString`: `"` and `\` and the control
/// characters, as the short escapes or `\u00XX`.
pub fn json_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => {
                out.push_str("\\u00");
                let n = u32::from(c);
                out.push(
                    char::from_digit(n >> 4, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit(n & 15, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
            }
            c => out.push(c),
        }
    }
}

/// `URLEncoder.encode(s, "UTF-8")`.
pub fn url_encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'*' | b'_' => {
                out.push(char::from(b));
            }
            b' ' => out.push('+'),
            b => {
                out.push('%');
                out.push(
                    char::from_digit(u32::from(b >> 4), 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit(u32::from(b & 15), 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
}

/// `Double.toString(d)`: the shortest digits that read back, in plain form
/// from 10^-3 up to 10^7 and in computerized scientific notation outside.
pub fn java_double(d: f64) -> String {
    if d.is_nan() {
        // ALLOC: the template, its params and its output, which the request limits bound.
        return "NaN".to_owned();
    }
    if d.is_infinite() {
        // ALLOC: the template, its params and its output, which the request limits bound.
        return if d > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if d == 0.0 {
        // ALLOC: the template, its params and its output, which the request limits bound.
        return if d.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    // `{:e}` gives the shortest round-trip digits: d.ddde[-]x.
    // ALLOC: the template, its params and its output, which the request limits bound.
    let sci = format!("{:e}", d.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    // ALLOC: the template, its params and its output, which the request limits bound.
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    // ALLOC: the template, its params and its output, which the request limits bound.
    let mut out = String::new();
    if d < 0.0 {
        out.push('-');
    }
    let abs = d.abs();
    if (1e-3..1e7).contains(&abs) {
        if exp >= 0 {
            let int_len = usize::try_from(exp).unwrap_or(0).saturating_add(1);
            // ALLOC: the template, its params and its output, which the request limits bound.
            let mut int_part = String::new();
            // ALLOC: the template, its params and its output, which the request limits bound.
            let mut frac = String::new();
            for (i, c) in digits.chars().enumerate() {
                if i < int_len {
                    int_part.push(c);
                } else {
                    frac.push(c);
                }
            }
            while int_part.len() < int_len {
                int_part.push('0');
            }
            if frac.is_empty() {
                frac.push('0');
            }
            out.push_str(&int_part);
            out.push('.');
            out.push_str(&frac);
        } else {
            out.push_str("0.");
            for _ in 0..exp.unsigned_abs().saturating_sub(1) {
                out.push('0');
            }
            out.push_str(&digits);
        }
    } else {
        let mut chars = digits.chars();
        out.push(chars.next().unwrap_or('0'));
        out.push('.');
        // ALLOC: the template, its params and its output, which the request limits bound.
        let rest: String = chars.collect();
        if rest.is_empty() {
            out.push('0');
        } else {
            out.push_str(&rest);
        }
        out.push('E');
        // ALLOC: the template, its params and its output, which the request limits bound.
        out.push_str(&exp.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_print_as_java_prints_them() {
        for (d, want) in [
            (1.5, "1.5"),
            (100.0, "100.0"),
            (1e7, "1.0E7"),
            (0.001, "0.001"),
            (0.000_99, "9.9E-4"),
            (123_456_789.123, "1.23456789123E8"),
            (-0.0, "-0.0"),
            (1.0e-5, "1.0E-5"),
            (9_999_999.0, "9999999.0"),
            (0.1, "0.1"),
            (-2.5e-7, "-2.5E-7"),
            (f64::INFINITY, "Infinity"),
        ] {
            assert_eq!(java_double(d), want, "{d}");
        }
    }

    #[test]
    fn strings_escape_as_jackson_escapes_them() {
        let mut out = String::new();
        json_escape("a\"b\\c\nd\u{1}\u{1f}/é\u{7f}", &mut out);
        assert_eq!(out, "a\\\"b\\\\c\\nd\\u0001\\u001F/é\u{7f}");
    }

    #[test]
    fn urls_encode_as_url_encoder_does() {
        let mut out = String::new();
        url_encode("a b+c~*-._é€/?", &mut out);
        assert_eq!(out, "a+b%2Bc%7E*-._%C3%A9%E2%82%AC%2F%3F");
    }
}
