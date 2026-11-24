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
    /// A partition-based `include`: the terms whose hash falls in `partition`
    /// of `num_partitions`.
    Partition {
        /// The zero-based partition asked for.
        partition: u32,
        /// How many partitions the terms divide into.
        num_partitions: u32,
    },
}

impl TermFilter {
    /// `"regex"` or `"set"`, as `IncludeExclude.merge` names it.
    fn method(&self) -> &'static str {
        match self {
            TermFilter::Regex(_) => "regex",
            TermFilter::Set(_) => "set",
            TermFilter::Partition { .. } => "partition",
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

    /// The name of the first option set, which a numeric field does not
    /// serve.
    pub fn first_set(&self) -> Option<&'static str> {
        if self.include.is_some() {
            Some("include")
        } else if self.exclude.is_some() {
            Some("exclude")
        } else if self.missing.is_some() {
            Some("missing")
        } else {
            None
        }
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
        // The include side of the merge: the new filter when it is the
        // late `include`, else the earlier one.
        let include = if late_include { Some(new) } else { earlier };
        let excluded = if late_include {
            earlier.is_some()
        } else {
            true
        };
        if excluded && matches!(include, Some(TermFilter::Partition { .. })) {
            // ALLOC: error path; one message.
            return Err(DslError::illegal(
                "Cannot specify any excludes when using a partition-based include".to_owned(),
            ));
        }
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
/// and an array is a set; an object is a partition-based `include`.
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
        Kind::StartObject if key == "include" => parse_partition(p),
        other => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("Unrecognized token for an {key} [{}]", other.name()),
        )),
    }
}

/// A partition-based `include`'s object: `partition` and `num_partitions`,
/// both whole numbers, with `IncludeExclude`'s checks.
fn parse_partition(p: &mut Parser<'_>) -> Result<TermFilter, DslError> {
    let mut partition: Option<i64> = None;
    let mut num_partitions: Option<i64> = None;
    let mut name = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => name = p.text(),
            Some(Kind::Number | Kind::String) => {
                let slot = match name {
                    "partition" => &mut partition,
                    "num_partitions" => &mut num_partitions,
                    _ => return Err(unknown_parameter(name)),
                };
                let value = p.text().trim().parse::<i32>().map_err(|_| {
                    DslError::unsupported(
                        "a [partition] or [num_partitions] that is not an integer",
                    )
                })?;
                *slot = Some(i64::from(value));
            }
            Some(_) => return Err(unknown_parameter(name)),
        }
    }
    let Some(partition) = partition else {
        // ALLOC: error path; one message.
        return Err(DslError::illegal(
            "Missing [partition] parameter for partition-based include".to_owned(),
        ));
    };
    let Some(num_partitions) = num_partitions else {
        // ALLOC: error path; one message.
        return Err(DslError::illegal(
            "Missing [num_partitions] parameter for partition-based include".to_owned(),
        ));
    };
    if partition < 0 || partition >= num_partitions {
        // ALLOC: error path; one message.
        return Err(DslError::illegal(format!(
            "Partition must be >=0 and < numPartition which is {num_partitions}"
        )));
    }
    Ok(TermFilter::Partition {
        partition: u32::try_from(partition).unwrap_or(0),
        num_partitions: u32::try_from(num_partitions).unwrap_or(1),
    })
}

fn unknown_parameter(name: &str) -> DslError {
    DslError::Failure(crate::failure::Failure::opensearch(
        "parse_exception",
        // ALLOC: error path; one message.
        format!("Unknown parameter in Include/Exclude clause: {name}"),
    ))
}
