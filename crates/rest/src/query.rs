//! Percent-decoding and the query string.
//!
//! Ported from `server/src/main/java/org/opensearch/rest/RestUtils.java`
//! (`decodeQueryString`, `decodeComponent`) at huyz0/OpenSearch@49f238ed,
//! keeping its quirks: `&` and `;` both separate parameters, `#` ends the query
//! string, `%%` decodes to `%`, a later repeat of a name replaces the earlier
//! one, and `+` is a space in the query string but not in the path.
//!
//! Parameters are kept as ranges into the request line, in a list allocated
//! once per worker, and decoded only when read: a component without `%` (or
//! `+` in the query) is borrowed, not copied.

use std::borrow::Cow;
use std::ops::Range;

use crate::error::RestError;

/// Decodes a percent-encoded URL component, refusing a malformed escape with
/// the message Java's `RestUtils#decode` throws.
///
/// Java decodes into a `String` through UTF-8 and replaces invalid
/// sequences; this refuses them instead.
pub fn decode_component(s: &str, plus_as_space: bool) -> Result<Cow<'_, str>, RestError> {
    let bytes = s.as_bytes();
    if !bytes
        .iter()
        .any(|&b| b == b'%' || (plus_as_space && b == b'+'))
    {
        return Ok(Cow::Borrowed(s));
    }
    // ALLOC: one component of the request line, which is bounded by the
    // initial line limit; decoding only shrinks it.
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        i = i.saturating_add(1);
        match b {
            b'+' if plus_as_space => out.push(b' '),
            b'%' => {
                let Some(&hi) = bytes.get(i) else {
                    // ALLOC: error path; bounded by the request line or a fixed message.
                    return Err(RestError::illegal_argument(format!(
                        "unterminated escape sequence at end of string: {s}"
                    )));
                };
                i = i.saturating_add(1);
                if hi == b'%' {
                    out.push(b'%');
                    continue;
                }
                let Some(&lo) = bytes.get(i) else {
                    // ALLOC: error path; bounded by the request line or a fixed message.
                    return Err(RestError::illegal_argument(format!(
                        "partial escape sequence at end of string: {s}"
                    )));
                };
                i = i.saturating_add(1);
                match (hex(hi), hex(lo)) {
                    (Some(h), Some(l)) => out.push((h << 4) | l),
                    _ => {
                        // ALLOC: error path; bounded by the request line or a fixed message.
                        return Err(RestError::illegal_argument(format!(
                            "invalid escape sequence `%{}{}' at index {} of: {s}",
                            char::from(hi),
                            char::from(lo),
                            i.saturating_sub(3)
                        )));
                    }
                }
            }
            other => out.push(other),
        }
    }
    String::from_utf8(out)
        .map(Cow::Owned)
        // ALLOC: error path; bounded by the request line or a fixed message.
        .map_err(|_| RestError::illegal_argument(format!("invalid UTF-8 after decoding: {s}")))
}

fn hex(b: u8) -> Option<u8> {
    char::from(b)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

/// The query string's parameters, as ranges into it.
#[derive(Debug)]
pub(crate) struct QueryParams {
    pairs: Vec<(Range<usize>, Range<usize>)>,
    max: usize,
}

impl QueryParams {
    /// Room for `max` parameters, allocated once.
    pub(crate) fn with_capacity(max: usize) -> Self {
        Self {
            // ALLOC: once per worker, `max` pairs, reused for every request.
            pairs: Vec::with_capacity(max),
            max,
        }
    }

    /// Splits `query` (the text after `?`) into parameters, checking that
    /// every name and value decodes. More than `max` parameters is refused.
    pub(crate) fn parse(&mut self, query: &str, path: &str) -> Result<(), RestError> {
        self.pairs.clear();
        let end = query.find('#').unwrap_or(query.len());
        let bytes = &query.as_bytes()[..end];
        let mut name: Option<Range<usize>> = None;
        let mut pos = 0;
        for (i, &c) in bytes.iter().enumerate() {
            if c == b'=' && name.is_none() {
                if pos != i {
                    name = Some(pos..i);
                }
                pos = i.saturating_add(1);
            } else if c == b'&' || c == b';' {
                match name.take() {
                    None if pos != i => self.add(query, pos..i, i..i, path)?,
                    Some(n) => self.add(query, n, pos..i, path)?,
                    None => {}
                }
                pos = i.saturating_add(1);
            }
        }
        match name {
            None if pos != end => self.add(query, pos..end, end..end, path)?,
            Some(n) => self.add(query, n, pos..end, path)?,
            None => {}
        }
        Ok(())
    }

    fn add(
        &mut self,
        query: &str,
        name: Range<usize>,
        value: Range<usize>,
        path: &str,
    ) -> Result<(), RestError> {
        let n = decode_component(&query[name.clone()], true)?;
        decode_component(&query[value.clone()], true)?;
        if let Some(i) = self
            .pairs
            .iter()
            .position(|(r, _)| decode_component(&query[r.clone()], true).is_ok_and(|e| e == n))
        {
            self.pairs.remove(i);
        }
        if self.pairs.len() == self.max {
            // ALLOC: error path; bounded by the request line or a fixed message.
            return Err(RestError::illegal_argument(format!(
                "request [{path}] has more than [{}] parameters",
                self.max
            )));
        }
        self.pairs.push((name, value));
        Ok(())
    }

    /// The decoded value of `name`, or `None` when absent.
    pub(crate) fn get<'q>(&self, query: &'q str, name: &str) -> Option<Cow<'q, str>> {
        self.pairs.iter().find_map(|(n, v)| {
            let decoded = decode_component(&query[n.clone()], true).ok()?;
            if decoded == name {
                decode_component(&query[v.clone()], true).ok()
            } else {
                None
            }
        })
    }

    /// Every parameter's decoded name, in order of appearance.
    pub(crate) fn names<'q>(&'q self, query: &'q str) -> impl Iterator<Item = Cow<'q, str>> + 'q {
        self.pairs
            .iter()
            .filter_map(move |(n, _)| decode_component(&query[n.clone()], true).ok())
    }

    /// How many parameters there are.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pairs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(e: &RestError) -> String {
        e.as_exception()
            .map(|x| x.reason().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn components_decode_like_rest_utils() {
        assert!(matches!(
            decode_component("plain", true),
            Ok(Cow::Borrowed("plain"))
        ));
        assert_eq!(
            decode_component("a%20b", false).ok().as_deref(),
            Some("a b")
        );
        assert_eq!(decode_component("a+b", true).ok().as_deref(), Some("a b"));
        assert_eq!(decode_component("a+b", false).ok().as_deref(), Some("a+b"));
        assert_eq!(
            decode_component("100%%", false).ok().as_deref(),
            Some("100%")
        );
        assert_eq!(decode_component("%C3%A9", false).ok().as_deref(), Some("é"));
        assert_eq!(decode_component("%c3%a9", false).ok().as_deref(), Some("é"));
    }

    #[test]
    fn malformed_escapes_are_refused_with_java_messages() {
        let e = decode_component("ab%", false).expect_err("unterminated");
        assert_eq!(
            reason(&e),
            "unterminated escape sequence at end of string: ab%"
        );
        let e = decode_component("ab%4", false).expect_err("partial");
        assert_eq!(reason(&e), "partial escape sequence at end of string: ab%4");
        let e = decode_component("ab%zz", false).expect_err("invalid");
        assert_eq!(
            reason(&e),
            "invalid escape sequence `%zz' at index 2 of: ab%zz"
        );
        assert_eq!(e.status(), 400);
        let e = decode_component("%ff", false).expect_err("not utf-8");
        assert_eq!(reason(&e), "invalid UTF-8 after decoding: %ff");
    }

    fn parse(q: &str) -> Vec<(String, String)> {
        let mut p = QueryParams::with_capacity(16);
        p.parse(q, "/p").expect("parses");
        p.pairs
            .iter()
            .map(|(n, v)| (q[n.clone()].to_owned(), q[v.clone()].to_owned()))
            .collect()
    }

    #[test]
    fn query_strings_split_like_decode_query_string() {
        let pairs = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(parse("a=1&b=2"), pairs(&[("a", "1"), ("b", "2")]));
        assert_eq!(parse("a=1;b"), pairs(&[("a", "1"), ("b", "")]));
        assert_eq!(parse("pretty&a="), pairs(&[("pretty", ""), ("a", "")]));
        assert_eq!(parse("a=1#frag&b=2"), pairs(&[("a", "1")]));
        assert_eq!(parse("=x"), pairs(&[("x", "")]));
        assert_eq!(parse("&&a=b=c"), pairs(&[("a", "b=c")]));
        assert_eq!(parse(""), pairs(&[]));
        assert_eq!(parse("a=1&a=2"), pairs(&[("a", "2")]));
    }

    #[test]
    fn values_decode_on_read_and_repeats_keep_the_last() {
        let q = "q=a+b%21&size=1&size=2&x%5Fy=z";
        let mut p = QueryParams::with_capacity(8);
        p.parse(q, "/p").expect("parses");
        assert_eq!(p.get(q, "q").as_deref(), Some("a b!"));
        assert_eq!(p.get(q, "size").as_deref(), Some("2"));
        assert_eq!(p.get(q, "x_y").as_deref(), Some("z"));
        assert_eq!(p.get(q, "missing"), None);
        assert_eq!(p.names(q).collect::<Vec<_>>(), ["q", "size", "x_y"]);
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn a_malformed_parameter_fails_the_parse() {
        let mut p = QueryParams::with_capacity(8);
        let e = p.parse("a=%zz", "/p").expect_err("bad value");
        assert_eq!(
            reason(&e),
            "invalid escape sequence `%zz' at index 0 of: %zz"
        );
        assert!(p.parse("%=1", "/p").is_err());
    }

    #[test]
    fn the_parameter_limit_holds_at_max_and_refuses_one_more() {
        let mut p = QueryParams::with_capacity(3);
        p.parse("a&b&c", "/p").expect("exactly the limit");
        assert_eq!(p.len(), 3);
        p.parse("a&b&c&a", "/p")
            .expect("a repeat replaces, it does not add");
        let e = p.parse("a&b&c&d", "/p").expect_err("one past the limit");
        assert_eq!(reason(&e), "request [/p] has more than [3] parameters");
        assert_eq!(p.pairs.capacity(), 3, "never grows past its allocation");
    }
}
