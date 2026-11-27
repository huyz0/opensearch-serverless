//! `significant_terms`' body: `SignificantTermsAggregationBuilder`'s
//! `ObjectParser` and its setters' checks at OpenSearch 3.10.0, and
//! `SignificantTermsAggregatorFactory`'s shard size.

use crate::aggs::bucket::significant::{Heuristic, SignificantSpec};
use crate::aggs::bucket::terms::suggested_shard_size;
use crate::aggs::metric::parse::long_value;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{int_value, suggest};

/// Every key `significant_terms` declares.
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
    "include",
    "exclude",
    "execution_hint",
    "background_filter",
    "jlh",
    "percentage",
    "mutual_information",
    "chi_square",
    "gnd",
    "script_heuristic",
];

/// The field and options of a `significant_terms` body, the parser on its
/// start.
pub fn parse_significant_body(
    p: &mut Parser<'_>,
    name: &str,
) -> Result<(String, SignificantSpec), DslError> {
    let mut field: Option<String> = None;
    let mut spec = SignificantSpec::default();
    let mut shard_size: Option<usize> = None;
    let mut shard_min = 0u64;
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
                &format!(
                    "[significant_terms] unknown field [{key}]{}",
                    suggest(key, KEYS)
                ),
                None,
            ));
        }
        if !matches!(
            key,
            "field"
                | "size"
                | "shard_size"
                | "min_doc_count"
                | "shard_min_doc_count"
                | "jlh"
                | "percentage"
        ) {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [significant_terms] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" => kind == Kind::String,
            "jlh" | "percentage" => kind == Kind::StartObject,
            _ => matches!(kind, Kind::Number | Kind::String),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[significant_terms] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[significant_terms] failed to parse field [{key}]"),
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
                    spec.size = n;
                } else {
                    shard_size = Some(n);
                }
            }
            "min_doc_count" | "shard_min_doc_count" => {
                let n = long_value(kind, p.text()).map_err(|e| failed(p, e))?;
                let setter = if key == "min_doc_count" {
                    "minDocCount"
                } else {
                    "shardMinDocCount"
                };
                let n = u64::try_from(n).map_err(|_| {
                    failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "[{setter}] must be greater than or equal to 0. Found [{n}] in \
                                 [{name}]"
                            ),
                        ),
                    )
                })?;
                if key == "min_doc_count" {
                    spec.min_doc_count = n;
                } else {
                    shard_min = n;
                }
            }
            _ => {
                // A heuristic is an empty object.
                if p.next_token() != Some(Kind::EndObject) {
                    return Err(DslError::illegal(
                        // ALLOC: error path; one message.
                        format!(
                            "failed to parse [{key}] significance heuristic. expected an empty object"
                        ),
                    ));
                }
                spec.heuristic = if key == "jlh" {
                    Heuristic::Jlh
                } else {
                    Heuristic::Percentage
                };
            }
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    // `SignificantTermsAggregatorFactory`: no shard size asked for is twice
    // the suggestion, and no shard keeps fewer than the size.
    spec.shard_size = shard_size
        .unwrap_or_else(|| suggested_shard_size(spec.size).saturating_mul(2))
        .max(spec.size);
    // `ensureValidity`: a shard never asks for more than the answer does.
    spec.shard_min_doc_count = shard_min.min(spec.min_doc_count);
    Ok((field, spec))
}
