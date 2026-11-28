//! `sort`: `SortBuilder.fromXContent` for field and score sorts.
//!
//! Ported from `SortBuilder.fromXContent`, `#parseCompoundSortField` and
//! `#fieldOrScoreSort` (server/src/main/java/org/opensearch/search/sort/SortBuilder.java),
//! with `FieldSortBuilder.PARSER` and `ScoreSortBuilder.PARSER` read for
//! `order` only, at OpenSearch 3.10.0. A field sort is ascending by default,
//! a score sort descending.

use crate::dsl::error::DslError;
use crate::dsl::queries::function_score_script::{ScriptScore, object_with};
use crate::dsl::query::{Query, parse_inner};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;

/// The most sort keys a request may name.
pub const MAX_SORT_KEYS: usize = 16;

/// The nested object a sort key orders its parents by the children of
/// (`NestedSortBuilder`): the path, and the filter the children match.
#[derive(Clone, Debug, PartialEq)]
pub struct NestedSort {
    /// The nested object's full path.
    pub path: String,
    /// What a child must match to count; every child counts without one.
    pub filter: Option<Query>,
}

/// One sort key.
#[derive(Clone, Debug, PartialEq)]
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
        /// The nested object whose children give the value, if one is named.
        nested: Option<NestedSort>,
    },
    /// By a script's number (`ScriptSortBuilder`, `type: number`).
    Script {
        /// The expression.
        script: ScriptScore,
        /// Lowest first.
        ascending: bool,
    },
    /// By the distance from origins (`GeoDistanceSortBuilder`).
    GeoDistance(Box<crate::dsl::sort_geo::GeoDistanceSort>),
}

impl SortKey {
    /// `fieldOrScoreSort`: `_score` is a score sort, anything else a field.
    pub(crate) fn named(name: &str) -> Self {
        if name == "_score" {
            SortKey::Score { ascending: false }
        } else {
            SortKey::Field {
                // ALLOC: one name a sort key, at most MAX_SORT_KEYS.
                field: name.to_owned(),
                ascending: true,
                nested: None,
            }
        }
    }

    pub(crate) fn with_order(self, ascending: bool) -> Self {
        match self {
            SortKey::Score { .. } => SortKey::Score { ascending },
            SortKey::Field { field, nested, .. } => SortKey::Field {
                field,
                ascending,
                nested,
            },
            SortKey::Script { script, .. } => SortKey::Script { script, ascending },
            SortKey::GeoDistance(mut sort) => {
                sort.ascending = ascending;
                SortKey::GeoDistance(sort)
            }
        }
    }
}

/// `SortOrder.fromString`.
pub(crate) fn order(text: &str) -> Result<bool, DslError> {
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

pub(crate) fn push(keys: &mut Vec<SortKey>, key: SortKey) -> Result<(), DslError> {
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
            Some(Kind::StartObject) if name == "_script" => push(keys, script_key(p)?)?,
            Some(Kind::StartObject) if name == "_geo_distance" => {
                push(keys, crate::dsl::sort_geo::parse(p)?)?;
            }
            Some(Kind::String) => push(keys, SortKey::named(name).with_order(order(p.text())?))?,
            _ => push(keys, options(p, name)?)?,
        }
    }
}

/// `ScriptSortBuilder.fromXContent`, on the object's start: `type` (`number`),
/// `script` and `order`.
fn script_key(p: &mut Parser<'_>) -> Result<SortKey, DslError> {
    let (mut script, mut ascending, mut number) = (None, true, false);
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        let name = p.text().to_owned();
        match (name.as_str(), p.next_token()) {
            ("type", Some(Kind::String)) if p.text() == "number" => number = true,
            ("type", Some(Kind::String)) => {
                return Err(DslError::unsupported("[_script] sort of type [string]"));
            }
            ("order", Some(Kind::String)) => ascending = order(p.text())?,
            ("script", Some(Kind::StartObject)) => {
                script = Some(object_with(p, crate::painless_switch::native_compile())?);
            }
            ("script", Some(Kind::String)) => {
                return Err(DslError::illegal(
                    "[_script] runs only lang [expression] on this node, and a script given as text is painless",
                ));
            }
            ("mode" | "nested", _) => {
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!("[_script] sort [{name}]")));
            }
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::illegal(format!(
                    "[_script] sort unknown field [{name}]"
                )));
            }
        }
    }
    if !number {
        return Err(DslError::illegal("[_script] sort requires a [type]"));
    }
    let script = script.ok_or_else(|| DslError::illegal("[_script] sort requires a [script]"))?;
    Ok(SortKey::Script { script, ascending })
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
    // `nested_path` and `nested_filter` are one form, `nested` the other.
    let (mut legacy, mut modern) = (false, false);
    let (mut path, mut filter) = (None, None);
    let mut nested = None;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
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
        match field {
            "order" => key = key.with_order(order(p.text())?),
            "nested" => {
                modern = true;
                nested = nested_sort(p)?;
            }
            // ALLOC: one path a sort key.
            "nested_path" => {
                legacy = true;
                path = Some(p.text().to_owned());
            }
            "nested_filter" => {
                legacy = true;
                filter = Some(parse_inner(p)?);
            }
            _ => {
                p.skip_children();
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!("[{parser_name}] [{field}]")));
            }
        }
        if legacy && modern {
            return Err(DslError::illegal(
                "Setting both nested_path/nested_filter and nested not allowed",
            ));
        }
    }
    if let (SortKey::Field { nested: slot, .. }, Some(path)) = (&mut key, path) {
        *slot = Some(NestedSort { path, filter });
    } else if let SortKey::Field { nested: slot, .. } = &mut key {
        *slot = nested;
    }
    Ok(key)
}

/// `NestedSortBuilder.fromXContent`, with the parser on the value: the
/// path and the filter; `max_children` and a nested `nested` are refused.
/// A nested sort with no path is no nested sort, as `resolveNestedQuery`
/// has it.
fn nested_sort(p: &mut Parser<'_>) -> Result<Option<NestedSort>, DslError> {
    if p.current_token() != Some(Kind::StartObject) {
        return Err(DslError::illegal(
            "malformed nested sort format, must start with an object",
        ));
    }
    let (mut path, mut filter) = (None, None);
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {}
            Some(_) => {
                return Err(DslError::illegal(
                    "malformed nested sort format, only field names are allowed",
                ));
            }
        }
        let name = p.text();
        p.next_token();
        match name {
            // ALLOC: one path a sort key.
            "path" => path = Some(p.text().to_owned()),
            "filter" => filter = Some(parse_inner(p)?),
            "max_children" | "nested" => {
                p.skip_children();
                // ALLOC: error path; one message.
                return Err(DslError::unsupported(&format!("[nested] [{name}]")));
            }
            other => {
                // ALLOC: error path; one message.
                return Err(DslError::illegal(format!(
                    "malformed nested sort format, unknown field name [{other}]"
                )));
            }
        }
    }
    Ok(path.map(|path| NestedSort { path, filter }))
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
            nested: None,
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

    fn nested_of(doc: &str) -> Option<NestedSort> {
        match sort(doc).expect("parses").remove(0) {
            SortKey::Field { nested, .. } => nested,
            other => panic!("not a field sort: {other:?}"),
        }
    }

    #[test]
    fn a_nested_sort_names_its_path_and_filter_in_either_form() {
        let path = |p: &str| NestedSort {
            path: p.into(),
            filter: None,
        };
        assert_eq!(
            nested_of(r#"{"c.w":{"nested":{"path":"c"}}}"#),
            Some(path("c"))
        );
        assert_eq!(nested_of(r#"{"c.w":{"nested_path":"c"}}"#), Some(path("c")));
        assert_eq!(nested_of(r#"{"c.w":{"order":"desc"}}"#), None);
        // Without a path there is no nested sort, as `resolveNestedQuery` has it.
        assert_eq!(nested_of(r#"{"c.w":{"nested":{}}}"#), None);
        let filtered =
            nested_of(r#"{"c.w":{"nested":{"path":"c","filter":{"term":{"c.w":"x"}}}}}"#)
                .expect("nested");
        assert_eq!(filtered.path, "c");
        assert!(filtered.filter.is_some());
        let legacy =
            nested_of(r#"{"c.w":{"nested_path":"c","nested_filter":{"term":{"c.w":"x"}}}}"#)
                .expect("nested");
        assert_eq!(legacy, filtered);
    }

    #[test]
    fn a_malformed_nested_sort_fails_as_opensearch_does() {
        assert_eq!(
            reason(sort(r#"{"c.w":{"nested":{"path":"c"},"nested_path":"c"}}"#)),
            "Setting both nested_path/nested_filter and nested not allowed"
        );
        assert_eq!(
            reason(sort(
                r#"{"c.w":{"nested_filter":{"match_all":{}},"nested":{"path":"c"}}}"#
            )),
            "Setting both nested_path/nested_filter and nested not allowed"
        );
        assert_eq!(
            reason(sort(r#"{"c.w":{"nested":{"path":"c","pth":"d"}}}"#)),
            "malformed nested sort format, unknown field name [pth]"
        );
        assert_eq!(
            reason(sort(r#"{"c.w":{"nested":"c"}}"#)),
            "malformed nested sort format, must start with an object"
        );
    }

    #[test]
    fn what_a_nested_sort_does_not_serve_is_refused_by_name() {
        assert_eq!(
            reason(sort(r#"{"c.w":{"nested":{"path":"c","max_children":1}}}"#)),
            "[nested] [max_children] is not supported by this node yet"
        );
        assert_eq!(
            reason(sort(
                r#"{"c.w":{"nested":{"path":"c","nested":{"path":"d"}}}}"#
            )),
            "[nested] [nested] is not supported by this node yet"
        );
    }
}
