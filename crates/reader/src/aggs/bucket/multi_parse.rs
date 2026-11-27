//! `multi_terms`' body: `MultiTermsAggregationBuilder`'s `ObjectParser`, the
//! terms' `MultiValuesSourceFieldConfig` parser and the setters' checks, at
//! OpenSearch 3.10.0, and the factory's shard size.

use crate::aggs::bucket::BUCKET_LIMIT;
use crate::aggs::bucket::composite::{CompositeSpec, MissingOrder, SourceKind, SourceSpec};
use crate::aggs::bucket::terms::{TermsOrder, TermsSpec, suggested_shard_size};
use crate::aggs::bucket::terms_parse::parse_order;
use crate::aggs::metric::parse::long_value;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{int_value, suggest};

/// Every key `multi_terms` declares.
const KEYS: &[&str] = &[
    "terms",
    "size",
    "shard_size",
    "min_doc_count",
    "shard_min_doc_count",
    "show_term_doc_count_error",
    "order",
    "collect_mode",
];

/// Every key one of the terms declares.
const TERM_KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "time_zone",
    "include",
    "exclude",
];

/// The terms and options of a `multi_terms` body, the parser on its start:
/// the terms as the sources of the `composite` that counts their
/// combinations, and the options as a `terms`' (no key a source is kept: they
/// are named by their place).
pub fn parse_multi_body(
    p: &mut Parser<'_>,
    name: &str,
) -> Result<(CompositeSpec, TermsSpec), DslError> {
    let mut fields: Option<Vec<String>> = None;
    let mut size = 10usize;
    let mut shard_size: Option<usize> = None;
    let mut min_doc_count = 1u64;
    let mut shard_min = 0u64;
    let mut order = TermsOrder::CountDesc;
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
                &format!("[multi_terms] unknown field [{key}]{}", suggest(key, KEYS)),
                None,
            ));
        }
        if matches!(key, "collect_mode" | "show_term_doc_count_error") {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [multi_terms] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "terms" => kind == Kind::StartArray,
            "order" => kind == Kind::StartObject,
            _ => matches!(kind, Kind::Number | Kind::String),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[multi_terms] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[multi_terms] failed to parse field [{key}]"),
                Some(cause),
            )
        };
        match key {
            "terms" => {
                let found = parse_terms(p).map_err(|e| failed(p, e))?;
                if found.len() < 2 {
                    return Err(failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "multi term aggregation must has at least 2 terms. Found [{}] in \
                                 [{name}]",
                                found.len()
                            ),
                        ),
                    ));
                }
                fields = Some(found);
            }
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
                    min_doc_count = n;
                } else {
                    shard_min = n;
                }
            }
            _ => {
                let (parsed, by_sub) = parse_order(p)?;
                if by_sub.is_some() {
                    return Err(DslError::unsupported(
                        "an [order] by a sub-aggregation in a [multi_terms] aggregation",
                    ));
                }
                order = parsed;
            }
        }
    }
    let fields = fields.ok_or_else(|| {
        DslError::illegal(
            // ALLOC: error path; one message.
            format!("[terms] must not be null. Found null terms in [{name}]"),
        )
    })?;
    let shard_size = shard_size.unwrap_or(suggested_shard_size(size)).max(size);
    let shard_min_doc_count = shard_min.min(min_doc_count);
    if shard_min_doc_count > 0 && order != TermsOrder::CountDesc {
        // The shard keeps its best buckets before the minimum applies, which
        // only matches OpenSearch when the order is by count, descending.
        return Err(DslError::unsupported(
            "a [shard_min_doc_count] with an [order] other than [_count] descending in a \
             [multi_terms] aggregation",
        ));
    }
    let sources = fields
        .into_iter()
        .enumerate()
        .map(|(at, field)| SourceSpec {
            // ALLOC: one name a term, bounded by the body's recording.
            name: format!("t{at}"),
            field,
            kind: SourceKind::Terms,
            desc: false,
            missing_bucket: false,
            missing_order: MissingOrder::Default,
        })
        // ALLOC: one source a term, as above.
        .collect();
    Ok((
        CompositeSpec {
            sources,
            // Every combination, so the best are picked by the order: one
            // more than the limit tells the shard it overflowed.
            size: BUCKET_LIMIT.saturating_add(1),
            after: None,
        },
        TermsSpec {
            size,
            shard_size,
            min_doc_count,
            shard_min_doc_count,
            order,
            ..TermsSpec::default()
        },
    ))
}

/// The array of terms, the parser past its `[`: the field of each.
fn parse_terms(p: &mut Parser<'_>) -> Result<Vec<String>, DslError> {
    // ALLOC: one field name a term, bounded by the body's recording.
    let mut out = Vec::new();
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(out),
            Some(Kind::StartObject) => out.push(parse_term(p)?),
            Some(other) => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: error path; one message.
                    format!(
                        "Failed to parse object: expecting token of type [START_OBJECT] but \
                         found [{}]",
                        other.name()
                    ),
                ));
            }
        }
    }
}

/// One term's object, the parser past its `{`: its field.
fn parse_term(p: &mut Parser<'_>) -> Result<String, DslError> {
    let mut field: Option<String> = None;
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
        if !TERM_KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[term] unknown field [{key}]{}", suggest(key, TERM_KEYS)),
                None,
            ));
        }
        if key != "field" {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "a [{key}] in a term of a [multi_terms] aggregation"
            )));
        }
        if kind != Kind::String {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[term] field doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        // ALLOC: one field name a term, bounded by the body's recording.
        field = Some(p.text().to_owned());
    }
    field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })
}
