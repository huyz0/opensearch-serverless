//! A restore's rename: the pattern and replacement the Java applies with
//! `Matcher.replaceAll`, here over the `regex` crate. A pattern the crate
//! cannot compile (backreferences, lookaround) is refused, not
//! approximated.

use std::fmt::Write;

use regex::Regex;

use crate::route::resolve::Refusal;

/// How target names are made from source names.
#[derive(Debug, Clone)]
pub struct Rename {
    pattern: Regex,
    replacement: String,
}

impl Rename {
    /// A rename from the request's pattern and replacement texts.
    ///
    /// # Errors
    ///
    /// 400 `illegal_argument_exception` for a pattern this node cannot
    /// apply or a replacement the Java refuses.
    pub fn new(source: &str, with: &str) -> Result<Self, Refusal> {
        let compiled = Regex::new(source).map_err(|e| {
            // ALLOC: error path; one message.
            illegal(format!(
                "rename_pattern [{source}] is not a pattern this node can apply (no \
                 backreferences or lookaround): {e}"
            ))
        })?;
        let replacement = replacement(with, &compiled)?;
        Ok(Self {
            pattern: compiled,
            replacement,
        })
    }

    /// The target name for `source`: every match of the pattern replaced.
    #[must_use]
    pub fn apply(&self, source: &str) -> String {
        self.pattern
            .replace_all(source, self.replacement.as_str())
            .into_owned()
    }
}

fn illegal(reason: String) -> Refusal {
    Refusal {
        status: 400,
        kind: "illegal_argument_exception",
        reason,
    }
}

/// Java's `Matcher.replaceAll` replacement, in the `regex` crate's: `$n` and
/// `${name}` name a group, `\x` is `x`, and any other `$` or a trailing `\`
/// is refused as Java refuses it.
fn replacement(java: &str, pattern: &Regex) -> Result<String, Refusal> {
    let groups = pattern.captures_len().saturating_sub(1);
    // ALLOC: one replacement, bounded by the 1 MiB body.
    let mut out = String::with_capacity(java.len().saturating_add(8));
    let mut chars = java.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('$') => out.push_str("$$"),
                Some(next) => out.push(next),
                None => {
                    return Err(illegal(
                        // ALLOC: error path; one message.
                        "character to be escaped is missing in rename_replacement".to_owned(),
                    ));
                }
            },
            '$' => match chars.peek().copied() {
                Some('{') => {
                    chars.next();
                    // ALLOC: one group name, bounded by the 1 MiB body.
                    let mut name = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(n) => name.push(n),
                            None => {
                                return Err(illegal(
                                    "named capturing group is missing trailing '}' in rename_replacement"
                                        // ALLOC: error path; one message.
                                        .to_owned(),
                                ));
                            }
                        }
                    }
                    if !pattern.capture_names().flatten().any(|n| n == name) {
                        // ALLOC: error path; one message.
                        return Err(illegal(format!(
                            "no group with name {{{name}}} in rename_pattern"
                        )));
                    }
                    let _ = write!(out, "${{{name}}}");
                }
                Some(d) if d.is_ascii_digit() => {
                    chars.next();
                    let mut number =
                        usize::from(u8::try_from(d).unwrap_or(b'0').saturating_sub(b'0'));
                    if number > groups {
                        // ALLOC: error path; one message.
                        return Err(illegal(format!("No group {number} in rename_pattern")));
                    }
                    // More digits are taken while the group still exists.
                    while let Some(next) = chars.peek().copied().filter(char::is_ascii_digit) {
                        let more = number.saturating_mul(10).saturating_add(usize::from(
                            u8::try_from(next).unwrap_or(b'0').saturating_sub(b'0'),
                        ));
                        if more > groups {
                            break;
                        }
                        number = more;
                        chars.next();
                    }
                    let _ = write!(out, "${{{number}}}");
                }
                _ => {
                    return Err(illegal(
                        // ALLOC: error path; one message.
                        "Illegal group reference in rename_replacement".to_owned(),
                    ));
                }
            },
            other => out.push(other),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_replaces_every_match() {
        let rename = Rename::new("logs-(.+)", "restored-$1").unwrap();
        assert_eq!(rename.apply("logs-2026"), "restored-2026");
        assert_eq!(rename.apply("metrics"), "metrics");
        assert_eq!(Rename::new("o", "0").unwrap().apply("foo"), "f00");
    }

    #[test]
    fn replacements_follow_javas_rules() {
        let apply = |pattern: &str, with: &str, name: &str| {
            Rename::new(pattern, with).map(|r| r.apply(name))
        };
        // Digits are taken while the group exists: $1 then a literal 0.
        assert_eq!(apply("(a)", "$10", "a").unwrap(), "a0");
        assert_eq!(
            apply("(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)", "$10", "abcdefghij").unwrap(),
            "j"
        );
        assert_eq!(apply("(?P<x>a)", "<${x}>", "a").unwrap(), "<a>");
        assert_eq!(apply("a", r"\$", "a").unwrap(), "$");
        assert_eq!(apply("a", r"\n", "a").unwrap(), "n");
        for bad in ["$", "$x", "$ ", "$2", "${nope}", "${open", r"\"] {
            let r = apply("(a)", bad, "a").unwrap_err();
            assert_eq!(
                (r.status, r.kind),
                (400, "illegal_argument_exception"),
                "{bad}"
            );
        }
        // Lookaround and backreferences cannot be applied.
        for bad in ["(?=a)a", r"(a)\1", "("] {
            let r = apply(bad, "x", "a").unwrap_err();
            assert_eq!(
                (r.status, r.kind),
                (400, "illegal_argument_exception"),
                "{bad}"
            );
        }
    }
}
