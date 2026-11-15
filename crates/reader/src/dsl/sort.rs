//! `sort`: `SortBuilder.fromXContent` for field and score sorts.
//!
//! Ported from `SortBuilder.fromXContent`, `#parseCompoundSortField` and
//! `#fieldOrScoreSort` (server/src/main/java/org/opensearch/search/sort/SortBuilder.java),
//! with `FieldSortBuilder.PARSER` and `ScoreSortBuilder.PARSER` read for
//! `order` only, at OpenSearch 3.10.0. A field sort is ascending by default,
//! a score sort descending.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;

/// The most sort keys a request may name.
pub const MAX_SORT_KEYS: usize = 16;

/// One sort key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// By score.
    Score {
        /// Lowest first.
        ascending: bool,
    },
    /// By a field's values.
    Field {
        /// The field.
        field: String,
        /// Lowest first.
        ascending: bool,
    },
}

impl SortKey {
    /// `fieldOrScoreSort`: `_score` is a score sort, anything else a field.
    fn named(name: &str) -> Self {
        if name == "_score" {
            SortKey::Score { ascending: false }
        } else {
            SortKey::Field {
                // ALLOC: one name a sort key, at most MAX_SORT_KEYS.
                field: name.to_owned(),
                ascending: true,
            }
        }
    }

    fn with_order(self, ascending: bool) -> Self {
        match self {
            SortKey::Score { .. } => SortKey::Score { ascending },
            SortKey::Field { field, .. } => SortKey::Field { field, ascending },
        }
    }
}

/// `SortOrder.fromString`.
fn order(text: &str) -> Result<bool, DslError> {
    match text.to_ascii_uppercase().as_str() {
        "ASC" => Ok(true),
        "DESC" => Ok(false),
        other => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("No enum constant org.opensearch.search.sort.SortOrder.{other}"),
        )),
    }
}

/// `SortBuilder.fromXContent`: the parser is on the sort value's first
/// token (a string, an object or an array) and ends on its last.
pub fn parse_sort(p: &mut Parser<'_>) -> Result<Vec<SortKey>, DslError> {
    // ALLOC: one entry a sort key, at most MAX_SORT_KEYS.
    let mut keys = Vec::new();
    match p.current_token() {
        Some(Kind::StartArray) => loop {
            match p.next_token() {
                Some(Kind::EndArray) | None => break,
                Some(Kind::StartObject) => compound(p, &mut keys)?,
                Some(Kind::String) => push(&mut keys, SortKey::named(p.text()))?,
                _ => {
                    return Err(DslError::illegal(
                        "malformed sort format, within the sort array, an object, or an actual string are allowed",
                    ));
                }
            }
        },
        Some(Kind::String) => push(&mut keys, SortKey::named(p.text()))?,
        Some(Kind::StartObject) => compound(p, &mut keys)?,
        _ => {
            return Err(DslError::illegal(
                "malformed sort format, either start with array, object, or an actual string",
            ));
        }
    }
    Ok(keys)
}

fn push(keys: &mut Vec<SortKey>, key: SortKey) -> Result<(), DslError> {
    if keys.len() >= MAX_SORT_KEYS {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("too many sort keys, the maximum is [{MAX_SORT_KEYS}]"),
        ));
    }
    keys.push(key);
    Ok(())
}

/// `parseCompoundSortField`: `{"f": "desc", "g": {"order": "asc"}}`.
fn compound(p: &mut Parser<'_>, keys: &mut Vec<SortKey>) -> Result<(), DslError> {
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(()),
            Some(Kind::FieldName) => {}
            Some(_) => continue,
        }
        let name = p.text();
        match p.next_token() {
            Some(Kind::String) => push(keys, SortKey::named(name).with_order(order(p.text())?))?,
            _ => push(keys, options(p, name)?)?,
        }
    }
}

/// `FieldSortBuilder.fromXContent` / `ScoreSortBuilder.fromXContent`: an
/// object of options, of which `order` is served.
fn options(p: &mut Parser<'_>, name: &str) -> Result<SortKey, DslError> {
    let parser_name = if name == "_score" {
        "_score"
    } else {
        "field_sort"
    };
    let declared: &[&str] = if name == "_score" {
        &["order"]
    } else {
        &[
            "missing",
            "order",
            "mode",
            "sort_mode",
            "unmapped_type",
            "numeric_type",
            "format",
            "nested",
            "nested_path",
            "nested_filter",
        ]
    };
    if p.current_token() != Some(Kind::StartObject) {
        return Err(DslError::xcontent(
            p.location(),
            // ALLOC: error path; one message.
            &format!(
                "[{parser_name}] Expected START_OBJECT but was: {}",
                p.current_token().map_or("null", Kind::name)
            ),
            None,
        ));
    }
    let mut key = SortKey::named(name);
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(key),
            Some(Kind::FieldName) => {}
            Some(_) => continue,
        }
        let field = p.text();
        let at = p.location();
        if !declared.contains(&field) {
            return Err(DslError::xcontent(
                at,
                // ALLOC: error path; one message.
                &format!(
                    "[{parser_name}] unknown field [{field}]{}",
                    suggest(field, declared)
                ),
                None,
            ));
        }
        p.next_token();
        if field != "order" {
            p.skip_children();
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!("[{parser_name}] [{field}]")));
        }
        key = key.with_order(order(p.text())?);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::tokens::Recorder;
    use oss_rest::json::{Limits, Tokenizer};

    fn sort(doc: &str) -> Result<Vec<SortKey>, DslError> {
        let mut rec = Recorder::new(1 << 16);
        let mut t = Tokenizer::new(Limits::default());
        t.feed(doc.as_bytes(), &mut rec).expect("feeds");
        t.finish(&mut rec).expect("finishes");
        let mut p = rec.parser();
        p.next_token();
        parse_sort(&mut p)
    }

    fn field(f: &str, ascending: bool) -> SortKey {
        SortKey::Field {
            field: f.into(),
            ascending,
        }
    }

    #[test]
    fn every_form_of_sort_parses() {
        assert_eq!(sort("\"year\"").ok(), Some(vec![field("year", true)]));
        assert_eq!(
            sort("\"_score\"").ok(),
            Some(vec![SortKey::Score { ascending: false }])
        );
        assert_eq!(
            sort(r#"[{"year":{"order":"desc"}},"tag",{"_score":"asc"},{"t":"DESC","u":{}}]"#).ok(),
            Some(vec![
                field("year", false),
                field("tag", true),
                SortKey::Score { ascending: true },
                field("t", false),
                field("u", true),
            ])
        );
        assert_eq!(
            sort(r#"{"_score":{"order":"asc"}}"#).ok(),
            Some(vec![SortKey::Score { ascending: true }])
        );
    }

    fn reason(r: Result<Vec<SortKey>, DslError>) -> String {
        match r {
            Err(e) => e.into_failure().reason,
            Ok(keys) => panic!("parsed {keys:?}"),
        }
    }

    #[test]
    fn malformed_sorts_fail_as_opensearch_does() {
        assert!(reason(sort("[1]")).starts_with("malformed sort format, within"));
        assert!(reason(sort("1")).starts_with("malformed sort format, either"));
        assert_eq!(
            reason(sort(r#"{"y":"up"}"#)),
            "No enum constant org.opensearch.search.sort.SortOrder.UP"
        );
        assert_eq!(
            reason(sort(r#"{"y":{"ordr":"asc"}}"#)),
            "[1:7] [field_sort] unknown field [ordr] did you mean [order]?"
        );
        assert_eq!(
            reason(sort(r#"{"_score":{"mode":"min"}}"#)),
            "[1:12] [_score] unknown field [mode]"
        );
        assert_eq!(
            reason(sort(r#"{"y":{"missing":"_first"}}"#)),
            "[field_sort] [missing] is not supported by this node yet"
        );
        assert_eq!(
            reason(sort(r#"{"y":1}"#)),
            "[1:6] [field_sort] Expected START_OBJECT but was: VALUE_NUMBER"
        );
        let many = format!("[{}]", vec!["\"a\""; MAX_SORT_KEYS + 1].join(","));
        assert!(reason(sort(&many)).starts_with("too many sort keys"));
        let at_limit = format!("[{}]", vec!["\"a\""; MAX_SORT_KEYS].join(","));
        assert_eq!(sort(&at_limit).map(|k| k.len()).ok(), Some(MAX_SORT_KEYS));
    }
}
