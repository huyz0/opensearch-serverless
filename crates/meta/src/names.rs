//! What an index or alias name may be, before it becomes a key.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/Names.java`
//! (`validateIndexOrAlias`). A name is used verbatim as a key segment, so it
//! must never be a path: core's index-name rules, plus `#` and `:`, which
//! keys use to join a name to a shard and a repository to a snapshot. The
//! rules are checked in the Java's order and refused with its messages.

use std::fmt;

/// The longest name, in UTF-8 bytes: core's limit and the filesystem's.
pub const MAX_NAME_BYTES: usize = 255;

/// The characters core's `Strings.INVALID_FILENAME_CHARS` holds.
const INVALID_FILENAME_CHARS: [char; 10] = ['\\', '/', '*', '?', '"', '<', '>', '|', ' ', ','];

/// How the Java prints that set in its message: a `HashSet`'s order.
const INVALID_FILENAME_CHARS_TEXT: &str = r#"[ , ", *, \, <, |, ,, >, /, ?]"#;

/// The rule a name broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRule {
    /// The name is empty.
    Empty,
    /// The name is longer than [`MAX_NAME_BYTES`]; the length it has.
    TooLong(usize),
    /// The name holds a control character.
    Control,
    /// The name holds a character core refuses in a file name.
    FileNameChar,
    /// The name holds `#` or `:`.
    Separator,
    /// The name starts with `_`, `-` or `+`.
    Start,
    /// The name is `.` or `..`.
    Dots,
    /// The name is not lowercase.
    Uppercase,
}

/// A name refused, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameError {
    /// The name as given.
    pub name: String,
    /// The rule it broke.
    pub rule: NameRule,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = &self.name;
        match self.rule {
            NameRule::Empty => write!(f, "Invalid index or alias name [{name}], must not be empty"),
            NameRule::TooLong(len) => write!(
                f,
                "Invalid index or alias name [{name}], name is too long: ({len} > {MAX_NAME_BYTES})"
            ),
            NameRule::Control => write!(
                f,
                "Invalid index or alias name [{name}], must not contain control characters"
            ),
            NameRule::FileNameChar => write!(
                f,
                "Invalid index name [{name}], must not contain the following characters \
                 {INVALID_FILENAME_CHARS_TEXT}"
            ),
            NameRule::Separator => {
                write!(
                    f,
                    "Invalid index name [{name}], must not contain '#' or ':'"
                )
            }
            NameRule::Start => write!(
                f,
                "Invalid index name [{name}], must not start with '_', '-', or '+'"
            ),
            NameRule::Dots => write!(f, "Invalid index name [{name}], must not be '.' or '..'"),
            NameRule::Uppercase => write!(f, "Invalid index name [{name}], must be lowercase"),
        }
    }
}

impl std::error::Error for NameError {}

/// Refuses a name that cannot be an index or alias, naming the first rule
/// it breaks in the Java's order.
///
/// # Errors
///
/// [`NameError`] with the rule broken.
pub fn validate_index_or_alias(name: &str) -> Result<(), NameError> {
    rule_broken(name).map_or(Ok(()), |rule| {
        Err(NameError {
            // ALLOC: one copy of a refused name, for its message.
            name: name.to_owned(),
            rule,
        })
    })
}

fn rule_broken(name: &str) -> Option<NameRule> {
    if name.is_empty() {
        return Some(NameRule::Empty);
    }
    if name.len() > MAX_NAME_BYTES {
        return Some(NameRule::TooLong(name.len()));
    }
    // Java's Character.isISOControl is U+0000 to U+001F and U+007F to U+009F,
    // which is Rust's char::is_control.
    if name.chars().any(char::is_control) {
        return Some(NameRule::Control);
    }
    if name.contains(INVALID_FILENAME_CHARS) {
        return Some(NameRule::FileNameChar);
    }
    if name.contains(['#', ':']) {
        return Some(NameRule::Separator);
    }
    if name.starts_with(['_', '-', '+']) {
        return Some(NameRule::Start);
    }
    if name == "." || name == ".." {
        return Some(NameRule::Dots);
    }
    // Lowercase when no character changes under lowering, as
    // `name.equals(name.toLowerCase(Locale.ROOT))`.
    if name.chars().any(|c| !c.to_lowercase().eq([c])) {
        return Some(NameRule::Uppercase);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(name: &str) -> String {
        validate_index_or_alias(name).unwrap_err().to_string()
    }

    #[test]
    fn each_rule_is_refused_with_the_javas_message() {
        let long = "a".repeat(256);
        for (name, text) in [
            ("", "Invalid index or alias name [], must not be empty".to_owned()),
            (
                long.as_str(),
                format!("Invalid index or alias name [{long}], name is too long: (256 > 255)"),
            ),
            (
                "a\u{7}b",
                "Invalid index or alias name [a\u{7}b], must not contain control characters"
                    .to_owned(),
            ),
            (
                "a b",
                r#"Invalid index name [a b], must not contain the following characters [ , ", *, \, <, |, ,, >, /, ?]"#
                    .to_owned(),
            ),
            (
                "a#1",
                "Invalid index name [a#1], must not contain '#' or ':'".to_owned(),
            ),
            (
                "_a",
                "Invalid index name [_a], must not start with '_', '-', or '+'".to_owned(),
            ),
            (
                "..",
                "Invalid index name [..], must not be '.' or '..'".to_owned(),
            ),
            (
                "Logs",
                "Invalid index name [Logs], must be lowercase".to_owned(),
            ),
        ] {
            assert_eq!(message(name), text);
        }
    }

    #[test]
    fn every_case_of_each_rule_is_refused() {
        for (name, rule) in [
            ("é".repeat(128).as_str(), NameRule::TooLong(256)),
            ("\u{0}", NameRule::Control),
            ("\u{1f}", NameRule::Control),
            ("\u{7f}", NameRule::Control),
            ("\u{9f}", NameRule::Control),
            ("a\\b", NameRule::FileNameChar),
            ("a/b", NameRule::FileNameChar),
            ("a*", NameRule::FileNameChar),
            ("a?", NameRule::FileNameChar),
            ("a\"", NameRule::FileNameChar),
            ("a<", NameRule::FileNameChar),
            ("a>", NameRule::FileNameChar),
            ("a|", NameRule::FileNameChar),
            ("a,b", NameRule::FileNameChar),
            ("a:b", NameRule::Separator),
            ("-a", NameRule::Start),
            ("+a", NameRule::Start),
            (".", NameRule::Dots),
            ("aB", NameRule::Uppercase),
            ("é\u{C9}", NameRule::Uppercase),
            ("ǅ", NameRule::Uppercase),
            // Earlier rules win: a path is refused for its slash first.
            ("../A", NameRule::FileNameChar),
            ("_A", NameRule::Start),
        ] {
            assert_eq!(
                validate_index_or_alias(name).map_err(|e| e.rule),
                Err(rule),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_valid_name_passes() {
        for name in [
            "logs",
            "logs-2026.11",
            ".kibana",
            "a_b+c-d",
            "a..b",
            "été",
            "日本",
            "ς",
            "1",
            &"a".repeat(255),
            &"é".repeat(127),
        ] {
            assert_eq!(validate_index_or_alias(name), Ok(()), "{name}");
        }
    }
}
