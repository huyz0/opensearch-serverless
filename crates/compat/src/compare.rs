//! Structural comparison of two JSON documents, reporting each difference
//! with its JSON pointer and both values.

use std::fmt;

use serde_json::Value;

use crate::pointer::escape;

/// The most differences listed for one comparison; the rest are counted.
pub const MAX_DIFFERENCES: usize = 50;

/// Values longer than this, as compact JSON, are cut in a report.
const MAX_SHOWN_CHARS: usize = 120;

/// One difference between the expected and the actual document.
#[derive(Debug, Clone, PartialEq)]
pub enum Difference {
    /// Both have a value here and they differ (including in type).
    Changed {
        /// Where.
        pointer: String,
        /// The recorded value.
        expected: Value,
        /// The target's value.
        actual: Value,
    },
    /// The recording has a value here and the target does not.
    Missing {
        /// Where.
        pointer: String,
        /// The recorded value.
        expected: Value,
    },
    /// The target has a value the recording does not.
    Unexpected {
        /// Where.
        pointer: String,
        /// The target's value.
        actual: Value,
    },
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Changed {
                pointer,
                expected,
                actual,
            } => write!(
                f,
                "{pointer}: expected {}, got {}",
                shown(expected),
                shown(actual)
            ),
            Self::Missing { pointer, expected } => {
                write!(f, "{pointer}: missing (expected {})", shown(expected))
            }
            Self::Unexpected { pointer, actual } => {
                write!(f, "{pointer}: unexpected (got {})", shown(actual))
            }
        }
    }
}

/// Compact JSON, cut at [`MAX_SHOWN_CHARS`] characters.
fn shown(v: &Value) -> String {
    let text = v.to_string();
    match text.char_indices().nth(MAX_SHOWN_CHARS) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text,
    }
}

/// The differences found, at most [`MAX_DIFFERENCES`] listed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Differences {
    /// The first differences, in document order.
    pub listed: Vec<Difference>,
    /// How many more were found and not listed.
    pub omitted: usize,
}

impl Differences {
    /// No differences at all.
    pub fn is_empty(&self) -> bool {
        self.listed.is_empty() && self.omitted == 0
    }

    fn push(&mut self, d: Difference) {
        if self.listed.len() < MAX_DIFFERENCES {
            self.listed.push(d);
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }
}

impl fmt::Display for Differences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.listed {
            writeln!(f, "{d}")?;
        }
        if self.omitted > 0 {
            writeln!(f, "... and {} more", self.omitted)?;
        }
        Ok(())
    }
}

/// Compares `actual` against `expected`. Objects are compared member by
/// member regardless of order; arrays element by element, in order.
pub fn compare(expected: &Value, actual: &Value) -> Differences {
    let mut out = Differences::default();
    walk(expected, actual, &mut String::new(), &mut out);
    out
}

fn walk(expected: &Value, actual: &Value, at: &mut String, out: &mut Differences) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, ev) in e {
                let len = at.len();
                at.push('/');
                at.push_str(&escape(key));
                match a.get(key) {
                    Some(av) => walk(ev, av, at, out),
                    None => out.push(Difference::Missing {
                        pointer: at.clone(),
                        expected: ev.clone(),
                    }),
                }
                at.truncate(len);
            }
            for (key, av) in a.iter().filter(|(k, _)| !e.contains_key(*k)) {
                out.push(Difference::Unexpected {
                    pointer: format!("{at}/{}", escape(key)),
                    actual: av.clone(),
                });
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            for (i, ev) in e.iter().enumerate() {
                let len = at.len();
                at.push('/');
                at.push_str(&i.to_string());
                match a.get(i) {
                    Some(av) => walk(ev, av, at, out),
                    None => out.push(Difference::Missing {
                        pointer: at.clone(),
                        expected: ev.clone(),
                    }),
                }
                at.truncate(len);
            }
            for (i, av) in a.iter().enumerate().skip(e.len()) {
                out.push(Difference::Unexpected {
                    pointer: format!("{at}/{i}"),
                    actual: av.clone(),
                });
            }
        }
        (e, a) if e == a => {}
        (e, a) => out.push(Difference::Changed {
            pointer: if at.is_empty() {
                "/".to_owned()
            } else {
                at.clone()
            },
            expected: e.clone(),
            actual: a.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn lines(expected: Value, actual: Value) -> Vec<String> {
        compare(&expected, &actual)
            .listed
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn equal_documents_have_no_differences_whatever_the_member_order() {
        let a: Value = serde_json::from_str(r#"{"a": 1, "b": [1, {"c": null}]}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"b": [1, {"c": null}], "a": 1}"#).unwrap();
        let d = compare(&a, &b);
        assert!(d.is_empty());
        assert_eq!(d.to_string(), "");
    }

    #[test]
    fn a_changed_value_names_its_pointer_and_both_values() {
        assert_eq!(
            lines(
                json!({"hits": {"total": {"value": 2}}}),
                json!({"hits": {"total": {"value": 3}}})
            ),
            ["/hits/total/value: expected 2, got 3"]
        );
    }

    #[test]
    fn a_type_change_is_a_change() {
        assert_eq!(
            lines(json!({"a": "1"}), json!({"a": 1})),
            [r#"/a: expected "1", got 1"#]
        );
        assert_eq!(
            lines(json!({"a": [1]}), json!({"a": {"0": 1}})),
            [r#"/a: expected [1], got {"0":1}"#]
        );
    }

    #[test]
    fn a_root_change_is_reported_at_slash() {
        assert_eq!(lines(json!(1), json!(2)), ["/: expected 1, got 2"]);
    }

    #[test]
    fn missing_and_unexpected_members_are_reported() {
        assert_eq!(
            lines(json!({"a": 1, "b": "x"}), json!({"a": 1, "c": true})),
            [r#"/b: missing (expected "x")"#, "/c: unexpected (got true)"]
        );
    }

    #[test]
    fn arrays_compare_by_position_and_report_length_differences() {
        assert_eq!(
            lines(json!([1, 2, 3]), json!([1, 5])),
            ["/1: expected 2, got 5", "/2: missing (expected 3)"]
        );
        assert_eq!(
            lines(json!([1]), json!([1, {"x": 1}])),
            [r#"/1: unexpected (got {"x":1})"#]
        );
    }

    #[test]
    fn member_names_are_escaped_in_pointers() {
        assert_eq!(
            lines(json!({"a/b": 1}), json!({"a/b": 2})),
            ["/a~1b: expected 1, got 2"]
        );
        assert_eq!(
            lines(json!({}), json!({"x~y": 2})),
            ["/x~0y: unexpected (got 2)"]
        );
    }

    #[test]
    fn long_values_are_cut_in_the_report() {
        let long = "x".repeat(500);
        let line = &lines(json!({"a": long}), json!({"a": 1}))[0];
        assert!(line.contains("..."), "{line}");
        assert!(line.len() < 200, "{}", line.len());
    }

    #[test]
    fn listing_stops_at_the_cap_and_counts_the_rest() {
        let expected = Value::Array((0..60).map(Value::from).collect());
        let actual = Value::Array((100..160).map(Value::from).collect());
        let d = compare(&expected, &actual);
        assert_eq!(d.listed.len(), MAX_DIFFERENCES);
        assert_eq!(d.omitted, 10);
        assert!(!d.is_empty());
        assert!(d.to_string().ends_with("... and 10 more\n"));

        let at_cap = Value::Array((100..150).map(Value::from).collect());
        let d = compare(&Value::Array((0..50).map(Value::from).collect()), &at_cap);
        assert_eq!((d.listed.len(), d.omitted), (MAX_DIFFERENCES, 0));
    }
}
