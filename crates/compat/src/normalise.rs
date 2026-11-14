//! Normalisation: fields that legitimately vary between runs (`took`,
//! generated ids, node names) are replaced by a placeholder in both the
//! recorded and the actual response before they are compared.
//!
//! A rule never hides a structural difference: it only replaces values that
//! exist, so a field the target leaves out is still reported missing. A rule
//! that matches nothing in the recording is stale and reported, so rules
//! cannot outlive the field they were written for.

use serde_json::Value;

use crate::fixture::Rule;

/// What a normalised value is replaced with, on both sides.
pub const PLACEHOLDER: &str = "<normalised>";

/// Applies `rules` to the recorded document `expected` and the target's
/// document `actual`. Returns the pointers of rules that matched nothing in
/// `expected`.
pub fn normalise(rules: &[Rule], expected: &mut Value, actual: &mut Value) -> Vec<String> {
    let placeholder = Value::String(PLACEHOLDER.to_owned());
    let mut stale = Vec::new();
    for rule in rules {
        if rule.pointer.replace_all(expected, &placeholder) == 0 {
            stale.push(rule.pointer.to_string());
        }
        rule.pointer.replace_all(actual, &placeholder);
    }
    stale
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::compare::compare;
    use crate::pointer::Pointer;

    fn rule(pointer: &str) -> Rule {
        Rule {
            pointer: Pointer::parse(pointer).unwrap(),
            reason: "varies".into(),
        }
    }

    #[test]
    fn a_normalised_field_no_longer_differs() {
        let mut expected = json!({"body": {"took": 3, "hits": [{"_id": "a"}, {"_id": "b"}]}});
        let mut actual = json!({"body": {"took": 9, "hits": [{"_id": "x"}, {"_id": "y"}]}});
        let stale = normalise(
            &[rule("/body/took"), rule("/body/hits/*/_id")],
            &mut expected,
            &mut actual,
        );
        assert!(stale.is_empty());
        assert!(compare(&expected, &actual).is_empty());
        assert_eq!(expected["body"]["took"], json!(PLACEHOLDER));
    }

    #[test]
    fn a_field_missing_from_the_target_is_still_missing() {
        let mut expected = json!({"body": {"took": 3}});
        let mut actual = json!({"body": {}});
        assert!(normalise(&[rule("/body/took")], &mut expected, &mut actual).is_empty());
        let d = compare(&expected, &actual);
        assert_eq!(d.listed.len(), 1);
        assert_eq!(
            d.listed[0].to_string(),
            r#"/body/took: missing (expected "<normalised>")"#
        );
    }

    #[test]
    fn a_rule_that_matches_nothing_in_the_recording_is_stale() {
        let mut expected = json!({"body": {}});
        let mut actual = json!({"body": {"took": 1}});
        let stale = normalise(
            &[rule("/body/took"), rule("/body")],
            &mut expected,
            &mut actual,
        );
        assert_eq!(stale, ["/body/took"]);
    }

    #[test]
    fn other_fields_are_untouched() {
        let mut expected = json!({"body": {"took": 3, "total": 2}});
        let mut actual = json!({"body": {"took": 4, "total": 5}});
        normalise(&[rule("/body/took")], &mut expected, &mut actual);
        let d = compare(&expected, &actual);
        assert_eq!(d.listed.len(), 1);
        assert_eq!(d.listed[0].to_string(), "/body/total: expected 2, got 5");
    }
}
