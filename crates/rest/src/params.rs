//! Refusing query parameters a route does not take.
//!
//! Ported from `BaseRestHandler#handleRequest` and `#unrecognizedStrings`
//! (server/src/main/java/org/opensearch/rest/BaseRestHandler.java) at
//! huyz0/OpenSearch@49f238ed, with Lucene's `LevenshteinDistance`
//! (`org.apache.lucene.search.spell`) for the "did you mean" suggestions:
//! candidates scoring above 0.5 are offered, best first, ties by name.
//!
//! Java's candidates are the parameters the handler happened to read; here
//! they are the parameters the route declares, its path parameters, and the
//! edge's global ones, which is the same set for a handler that reads what it
//! declares.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::error::RestError;

/// Builds the refusal for `invalids` (sorted, as Java's `TreeSet` sorts
/// them), suggesting from `candidates`.
pub(crate) fn unrecognized<'a>(
    path: &str,
    invalids: &BTreeSet<&str>,
    candidates: impl Iterator<Item = &'a str> + Clone,
) -> RestError {
    let plural = if invalids.len() > 1 { "s" } else { "" };
    // ALLOC: error path; bounded by the request line and the route's
    // declared parameters.
    let mut message = format!("request [{path}] contains unrecognized parameter{plural}: ");
    for (i, invalid) in invalids.iter().enumerate() {
        // ALLOC: error path; at most the route's declared parameters.
        let mut scored: Vec<(f32, &str)> = candidates
            .clone()
            .map(|c| (levenshtein(invalid, c), c))
            .filter(|(d, _)| *d > 0.5)
            // ALLOC: error path; bounded by the request line or a fixed message.
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored.dedup_by(|a, b| a.1 == b.1);
        if i > 0 {
            message.push_str(", ");
        }
        let _ = write!(message, "[{invalid}]");
        match scored.as_slice() {
            [] => {}
            [(_, only)] => {
                let _ = write!(message, " -> did you mean [{only}]?");
            }
            many => {
                message.push_str(" -> did you mean any of [");
                for (j, (_, c)) in many.iter().enumerate() {
                    if j > 0 {
                        message.push_str(", ");
                    }
                    message.push_str(c);
                }
                message.push_str("]?");
            }
        }
    }
    RestError::illegal_argument(message)
}

/// Lucene's `LevenshteinDistance#getDistance`: `1 - edits / max(length)`
/// over UTF-16 code units, 1 for two empty strings and 0 when one is empty.
pub(crate) fn levenshtein(target: &str, other: &str) -> f32 {
    const MAX: usize = 256;
    let mut sa = [0u16; MAX];
    let mut so = [0u16; MAX];
    let n = fill(&mut sa, target);
    let m = fill(&mut so, other);
    let (sa, so) = (&sa[..n], &so[..m]);
    // With one side empty the table below gives 0: every unit of the other
    // is an edit, and the longest is that side.
    if n == 0 && m == 0 {
        return 1.0;
    }
    let mut p = [0u32; MAX + 1];
    let mut d = [0u32; MAX + 1];
    for (i, c) in p.iter_mut().enumerate().take(n.saturating_add(1)) {
        *c = u32::try_from(i).unwrap_or(u32::MAX);
    }
    for (j, &t_j) in so.iter().enumerate() {
        d[0] = u32::try_from(j.saturating_add(1)).unwrap_or(u32::MAX);
        for i in 1..=n {
            let cost = u32::from(sa[i.saturating_sub(1)] != t_j);
            let left = d[i.saturating_sub(1)].saturating_add(1);
            let up = p[i].saturating_add(1);
            let diag = p[i.saturating_sub(1)].saturating_add(cost);
            d[i] = left.min(up).min(diag);
        }
        std::mem::swap(&mut p, &mut d);
    }
    // Java computes `1.0f - ((float) p[n] / Math.max(m, n))` in float.
    #[allow(clippy::cast_precision_loss)]
    let (edits, longest) = (p[n] as f32, n.max(m) as f32);
    1.0 - edits / longest
}

/// Copies `s`'s UTF-16 units into `buf`, truncating past its length: a name
/// longer than 256 units is compared on its first 256, which can only make a
/// suggestion more likely, never refuse a valid parameter.
fn fill(buf: &mut [u16], s: &str) -> usize {
    let mut n: usize = 0;
    for (slot, unit) in buf.iter_mut().zip(s.encode_utf16()) {
        *slot = unit;
        n = n.saturating_add(1);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(e: &RestError) -> &str {
        e.as_exception().map(|x| x.reason()).unwrap_or_default()
    }

    #[test]
    fn levenshtein_matches_lucene() {
        assert_eq!(levenshtein("", ""), 1.0);
        assert_eq!(levenshtein("a", ""), 0.0);
        assert_eq!(levenshtein("", "a"), 0.0);
        assert_eq!(levenshtein("size", "size"), 1.0);
        assert_eq!(levenshtein("sise", "size"), 0.75);
        assert_eq!(levenshtein("kitten", "sitting"), 1.0 - 3.0 / 7.0);
        assert_eq!(levenshtein("abc", "xyz"), 0.0);
        // A supplementary character is two UTF-16 units, as in Java.
        assert_eq!(levenshtein("a\u{1F600}", "a"), 1.0 - 2.0 / 3.0);
    }

    #[test]
    fn one_unknown_parameter_with_one_suggestion() {
        let invalids = BTreeSet::from(["sise"]);
        let e = unrecognized("/idx/_search", &invalids, ["size", "from", "q"].into_iter());
        assert_eq!(e.status(), 400);
        assert_eq!(
            reason(&e),
            "request [/idx/_search] contains unrecognized parameter: [sise] -> did you mean [size]?"
        );
    }

    #[test]
    fn several_unknown_parameters_sorted_with_ranked_suggestions() {
        let invalids = BTreeSet::from(["zzz", "refrsh"]);
        let e = unrecognized(
            "/_bulk",
            &invalids,
            ["refresh", "refresh_x", "routing", "refresh"].into_iter(),
        );
        assert_eq!(
            reason(&e),
            "request [/_bulk] contains unrecognized parameters: [refrsh] -> did you mean any of [refresh, refresh_x]?, [zzz]"
        );
    }

    #[test]
    fn ties_are_broken_by_name() {
        let invalids = BTreeSet::from(["abc"]);
        let e = unrecognized("/p", &invalids, ["abe", "abd", "ab"].into_iter());
        assert_eq!(
            reason(&e),
            "request [/p] contains unrecognized parameter: [abc] -> did you mean any of [ab, abd, abe]?"
        );
        // A score of exactly 0.5 is not offered.
        let invalids = BTreeSet::from(["ab"]);
        let e = unrecognized("/p", &invalids, ["ac"].into_iter());
        assert_eq!(
            reason(&e),
            "request [/p] contains unrecognized parameter: [ab]"
        );
    }

    #[test]
    fn names_past_the_scratch_are_compared_on_their_prefix() {
        let long = "x".repeat(300);
        assert_eq!(levenshtein(&long, &long), 1.0);
    }

    #[test]
    fn an_empty_name_is_all_edits_from_any_other_and_alike_to_another_empty() {
        assert!((levenshtein("", "") - 1.0).abs() < f32::EPSILON);
        assert!(levenshtein("", "abc").abs() < f32::EPSILON);
        assert!(levenshtein("abc", "").abs() < f32::EPSILON);
        assert!((levenshtein("abc", "abd") - 2.0 / 3.0).abs() < 1e-6);
    }
}
