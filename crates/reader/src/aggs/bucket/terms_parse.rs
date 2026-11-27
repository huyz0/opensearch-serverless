//! `terms`' body: `TermsAggregationBuilder`'s `ObjectParser`
//! (`ValuesSourceAggregationBuilder.declareFields` and its own keys) and
//! its setters' checks, at OpenSearch 3.10.0, and `TermsAggregatorFactory`'s
//! shard size.

use crate::aggs::bucket::keyword_options::{KeywordOptions, parse_filter};
use crate::aggs::bucket::sub_order::{EMPTY_PATH, SubOrder, parse_path};
use crate::aggs::bucket::terms::{TermsOrder, TermsSpec, suggested_shard_size};
use crate::aggs::metric::parse::long_value;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, int_value, suggest};
use crate::failure::Failure;

/// Every key `terms` declares.
const KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "size",
    "shard_size",
    "min_doc_count",
    "shard_min_doc_count",
    "show_term_doc_count_error",
    "order",
    "include",
    "exclude",
    "execution_hint",
    "collect_mode",
];

/// The field and options of a `terms` body, the parser on its start.
pub fn parse_terms_body(p: &mut Parser<'_>, name: &str) -> Result<(String, TermsSpec), DslError> {
    let mut field: Option<String> = None;
    let mut size = 10usize;
    let mut shard_size: Option<usize> = None;
    let mut min_doc_count = 1u64;
    let mut shard_min = 0u64;
    let mut order = TermsOrder::CountDesc;
    let mut sub_order = None;
    let mut keyword = KeywordOptions::default();
    let mut show_error = false;
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        if !KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[terms] unknown field [{key}]{}", suggest(key, KEYS)),
                None,
            ));
        }
        let served = matches!(
            key,
            "field"
                | "size"
                | "shard_size"
                | "min_doc_count"
                | "shard_min_doc_count"
                | "order"
                | "include"
                | "exclude"
                | "missing"
                | "show_term_doc_count_error"
        );
        if !served {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [terms] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" => kind == Kind::String,
            "order" => matches!(kind, Kind::StartObject | Kind::StartArray),
            "include" | "exclude" => true,
            "missing" => matches!(kind, Kind::String | Kind::Number | Kind::Bool),
            "show_term_doc_count_error" => matches!(kind, Kind::Bool | Kind::String),
            _ => matches!(kind, Kind::Number | Kind::String),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[terms] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[terms] failed to parse field [{key}]"),
                Some(cause),
            )
        };
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            "size" | "shard_size" => {
                let n = int_value(p).map_err(|e| failed(p, e))?;
                let Some(n) = usize::try_from(n).ok().filter(|n| *n > 0) else {
                    let setter = if key == "size" { "size" } else { "shardSize" };
                    return Err(failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("[{setter}] must be greater than 0. Found [{n}] in [{name}]"),
                        ),
                    ));
                };
                if key == "size" {
                    size = n;
                } else {
                    shard_size = Some(n);
                }
            }
            "min_doc_count" => {
                let n = long_value(kind, p.text()).map_err(|e| failed(p, e))?;
                min_doc_count = u64::try_from(n).map_err(|_| {
                    failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "[minDocCount] must be greater than or equal to 0. Found [{n}] \
                                 in [{name}]"
                            ),
                        ),
                    )
                })?;
            }
            "shard_min_doc_count" => {
                let n = long_value(kind, p.text()).map_err(|e| failed(p, e))?;
                shard_min = u64::try_from(n).map_err(|_| {
                    failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "[shardMinDocCount] must be greater than or equal to 0. Found \
                                 [{n}] in [{name}]"
                            ),
                        ),
                    )
                })?;
            }
            "show_term_doc_count_error" => show_error = bool_value(p).map_err(|e| failed(p, e))?,
            "include" | "exclude" => {
                let filter = parse_filter(p, kind, key).map_err(|e| failed(p, e))?;
                keyword
                    .check_merge(key, &filter)
                    .map_err(|e| failed(p, e))?;
                if key == "include" {
                    keyword.include = Some(filter);
                } else {
                    keyword.exclude = Some(filter);
                }
            }
            "missing" => {
                if kind != Kind::String {
                    return Err(DslError::unsupported(
                        "a [missing] that is not a string in a [terms] aggregation",
                    ));
                }
                // ALLOC: one term an aggregation, bounded by the body's recording.
                keyword.missing = Some(p.text().to_owned());
            }
            _ => {
                if kind == Kind::StartArray {
                    return Err(DslError::unsupported(
                        "an [order] of several criteria in a [terms] aggregation",
                    ));
                }
                (order, sub_order) = parse_order(p)?;
            }
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    let key_ordered =
        sub_order.is_none() && matches!(order, TermsOrder::KeyAsc | TermsOrder::KeyDesc);
    // `TermsAggregatorFactory`: no shard size asked for is suggested from
    // the size unless the order is by key, and no shard keeps fewer than
    // the size.
    let shard_size = shard_size
        .unwrap_or(if key_ordered {
            size
        } else {
            suggested_shard_size(size)
        })
        .max(size);
    // `ensureValidity`: a shard never asks for more than the answer does.
    let shard_min_doc_count = shard_min.min(min_doc_count);
    if shard_min_doc_count > 0 && order != TermsOrder::CountDesc {
        // The engine picks a shard's buckets before the minimum applies, which
        // only matches OpenSearch when the order is by count, descending.
        return Err(DslError::unsupported(
            "a [shard_min_doc_count] with an [order] other than [_count] descending in a \
             [terms] aggregation",
        ));
    }
    Ok((
        field,
        TermsSpec {
            size,
            shard_size,
            min_doc_count,
            shard_min_doc_count,
            order,
            keyword,
            sub_order,
            show_error,
        },
    ))
}

/// `InternalOrder.Parser`'s one criterion over an object, the parser on its
/// start: `_count` or `_key` or the path of a sub-aggregation, `asc` or
/// `desc`. A path is read as `AggregationPath.parse` reads it, and its
/// target is checked when the shard runs.
pub(super) fn parse_order(p: &mut Parser<'_>) -> Result<(TermsOrder, Option<SubOrder>), DslError> {
    let mut by: Option<String> = None;
    let mut ascending = false;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                if by.is_some() {
                    return Err(DslError::unsupported(
                        "an [order] of several criteria in a [terms] aggregation",
                    ));
                }
                // ALLOC: one path an order, bounded by the body's recording.
                by = Some(p.text().to_owned());
            }
            Some(Kind::String) => {
                ascending = match p.text() {
                    "asc" => true,
                    "desc" => false,
                    other => {
                        return Err(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("Unknown order direction [{other}]"),
                        ));
                    }
                };
            }
            Some(other) => {
                return Err(DslError::illegal(
                    // ALLOC: error path; one message.
                    format!(
                        "Unexpected token [{}] in an order of a [terms] aggregation",
                        other.name()
                    ),
                ));
            }
        }
    }
    Ok(match (by.as_deref(), ascending) {
        (Some("_key" | "_term"), true) => (TermsOrder::KeyAsc, None),
        (Some("_key" | "_term"), false) => (TermsOrder::KeyDesc, None),
        (Some("_count") | None, true) => (TermsOrder::CountAsc, None),
        (Some("_count") | None, false) => (TermsOrder::CountDesc, None),
        (Some(path), _) => {
            sub_path(path)?;
            // The shard reads the order from the spec's `sub_order`.
            (
                TermsOrder::CountDesc,
                Some(SubOrder {
                    // ALLOC: one path an order, as above.
                    path: path.to_owned(),
                    ascending,
                }),
            )
        }
    })
}

/// `AggregationPath.parse`'s refusal of `path`, an
/// `AggregationExecutionException`.
fn sub_path(path: &str) -> Result<(), DslError> {
    parse_path(path).map(|_| ()).map_err(|message| {
        // No element at all is `AggregationPath`'s own
        // `IllegalArgumentException`, a bad element the exception of the
        // aggregation's execution.
        if message == EMPTY_PATH {
            return DslError::illegal(message);
        }
        DslError::Failure(Failure::opensearch(
            "aggregation_execution_exception",
            message,
        ))
    })
}
