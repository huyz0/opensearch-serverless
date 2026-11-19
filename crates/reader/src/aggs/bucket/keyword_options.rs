//! The options of a `terms` that only a keyword field reads: `include`,
//! `exclude` (`IncludeExclude`) and `missing`, as `TermsAggregationBuilder`
//! parses them at OpenSearch 3.10.0.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};

/// One side of `include` or `exclude`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermFilter {
    /// A Lucene regular expression the whole term must match.
    Regex(String),
    /// The terms, sorted and without repeats.
    Set(Vec<String>),
}

impl TermFilter {
    /// `"regex"` or `"set"`, as `IncludeExclude.merge` names it.
    fn method(&self) -> &'static str {
        match self {
            TermFilter::Regex(_) => "regex",
            TermFilter::Set(_) => "set",
        }
    }
}

/// What a keyword `terms` sets besides the options every `terms` has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeywordOptions {
    /// The terms that may be buckets; `None` for any.
    pub include: Option<TermFilter>,
    /// The terms that may not (evaluated after `include`).
    pub exclude: Option<TermFilter>,
    /// The term a document without a value counts under.
    pub missing: Option<String>,
}

impl KeywordOptions {
    /// Whether no keyword-only option is set.
    pub fn is_empty(&self) -> bool {
        self.include.is_none() && self.exclude.is_none() && self.missing.is_none()
    }

    /// Whether `include` or `exclude` is set.
    pub fn filters(&self) -> bool {
        self.include.is_some() || self.exclude.is_some()
    }

    /// `IncludeExclude.merge`'s check, when `key` (`include` or `exclude`)
    /// arrives after the other: the two must be both regular expressions or
    /// both sets. OpenSearch merges `(include, exclude)` for either key and
    /// names the first as the include, so a late `include` names its own
    /// method first.
    pub fn check_merge(&self, key: &str, new: &TermFilter) -> Result<(), DslError> {
        let late_include = key == "include";
        let earlier = if late_include {
            self.exclude.as_ref()
        } else {
            self.include.as_ref()
        };
        match earlier {
            Some(old) if old.method() != new.method() => {
                let (include, other) = if late_include {
                    (new.method(), old.method())
                } else {
                    (old.method(), new.method())
                };
                Err(DslError::illegal(
                    // ALLOC: error path; one message.
                    format!("Cannot mix a {include}-based include with a {other}-based method"),
                ))
            }
            _ => Ok(()),
        }
    }
}

/// `IncludeExclude.parseInclude` and `parseExclude`, the parser on the
/// value's first token (of kind `kind`): a string is a regular expression
/// and an array is a set. A partition-based include is not served.
pub fn parse_filter(p: &mut Parser<'_>, kind: Kind, key: &str) -> Result<TermFilter, DslError> {
    match kind {
        // ALLOC: one pattern an aggregation, bounded by the body's recording.
        Kind::String => Ok(TermFilter::Regex(p.text().to_owned())),
        Kind::StartArray => {
            let mut set: Vec<String> = Vec::new();
            loop {
                match p.next_token() {
                    Some(Kind::EndArray) | None => break,
                    Some(Kind::String | Kind::Number | Kind::Bool) => {
                        // ALLOC: one term a listed value, bounded by the body's recording.
                        set.push(p.text().to_owned());
                    }
                    Some(Kind::Null) => {
                        return Err(DslError::unsupported("a null in an [include] or [exclude]"));
                    }
                    Some(_) => {
                        return Err(DslError::Failure(
                            // ALLOC: error path; one message.
                            crate::failure::Failure::opensearch(
                                "parse_exception",
                                "Array elements in include/exclude clauses should be string values",
                            ),
                        ));
                    }
                }
            }
            set.sort();
            set.dedup();
            Ok(TermFilter::Set(set))
        }
        Kind::StartObject if key == "include" => Err(DslError::unsupported(
            "a partition-based [include] in a [terms] aggregation",
        )),
        other => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("Unrecognized token for an {key} [{}]", other.name()),
        )),
    }
}
