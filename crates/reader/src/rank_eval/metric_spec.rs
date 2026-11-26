//! The `metric` of a `_rank_eval` body: `precision`, `recall`,
//! `mean_reciprocal_rank` and `dcg`, parsed as the Java's constructing parsers do.

use super::metric::Metric;
use super::spec::{expect_object, int, unknown, wrong};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};

fn build_failed(p: &Parser<'_>, name: &str, cause: DslError) -> DslError {
    DslError::xcontent(
        p.location(),
        // ALLOC: error path; one message.
        &format!("[{name}] failed to build [{name}] after last required field arrived"),
        Some(cause),
    )
}

/// `{"precision": {...}}`, with the parser on the object's start.
pub(super) fn metric(p: &mut Parser<'_>) -> Result<Metric, DslError> {
    expect_object(p, "rank_eval", p.current_token())?;
    if p.next_token() != Some(Kind::FieldName) {
        return Err(DslError::xcontent(
            p.location(),
            "[rank_eval] expected a metric name",
            None,
        ));
    }
    // ALLOC: one name of the body, which the recording bounds.
    let name = p.text().to_owned();
    let at = p.location();
    let kind = p.next_token();
    let result = match name.as_str() {
        "precision" | "recall" | "mean_reciprocal_rank" => {
            let spec = if name == "mean_reciprocal_rank" {
                "reciprocal_rank"
            } else {
                &name
            };
            expect_object(p, spec, kind)?;
            let (mut threshold, mut ignore, mut k) = (None, None, None);
            while p.next_token() == Some(Kind::FieldName) {
                // ALLOC: one name of the body, which the recording bounds.
                let field = p.text().to_owned();
                let field_at = p.location();
                let value = p.next_token();
                match field.as_str() {
                    "relevant_rating_threshold" => {
                        threshold = Some(int(p, spec, &field, value)?);
                    }
                    "k" => k = Some(int(p, spec, &field, value)?),
                    "ignore_unlabeled" if name == "precision" => {
                        if value != Some(Kind::Bool) {
                            return Err(wrong(p, spec, &field, value));
                        }
                        ignore = Some(p.text() == "true");
                    }
                    _ => return Err(unknown(field_at, spec, &field)),
                }
            }
            let threshold = threshold.unwrap_or(1);
            let k = k.unwrap_or(10);
            if threshold < 0 {
                return Err(build_failed(
                    p,
                    spec,
                    DslError::illegal(
                        "Relevant rating threshold for precision must be positive integer.",
                    ),
                ));
            }
            if k <= 0 {
                return Err(build_failed(
                    p,
                    spec,
                    DslError::illegal("Window size k must be positive."),
                ));
            }
            match name.as_str() {
                "precision" => Metric::Precision {
                    threshold,
                    ignore_unlabeled: ignore.unwrap_or(false),
                    k,
                },
                "recall" => Metric::Recall { threshold, k },
                _ => Metric::ReciprocalRank { threshold, k },
            }
        }
        "dcg" => {
            expect_object(p, "dcg", kind)?;
            let (mut normalize, mut default_rating, mut k) = (None, None, None);
            while p.next_token() == Some(Kind::FieldName) {
                // ALLOC: one name of the body, which the recording bounds.
                let field = p.text().to_owned();
                let field_at = p.location();
                let value = p.next_token();
                match field.as_str() {
                    "normalize" => {
                        if value != Some(Kind::Bool) {
                            return Err(wrong(p, "dcg", &field, value));
                        }
                        normalize = Some(p.text() == "true");
                    }
                    "unknown_doc_rating" => default_rating = Some(int(p, "dcg", &field, value)?),
                    "k" => k = Some(int(p, "dcg", &field, value)?),
                    _ => return Err(unknown(field_at, "dcg", &field)),
                }
            }
            Metric::Dcg {
                normalize: normalize.unwrap_or(false),
                unknown_doc_rating: default_rating,
                k: k.unwrap_or(10),
            }
        }
        "expected_reciprocal_rank" => {
            // The Java names its parser `dcg`, and so do its errors.
            expect_object(p, "dcg", kind)?;
            let (mut max_relevance, mut default_rating, mut k) = (None, None, None);
            while p.next_token() == Some(Kind::FieldName) {
                // ALLOC: one name of the body, which the recording bounds.
                let field = p.text().to_owned();
                let field_at = p.location();
                let value = p.next_token();
                match field.as_str() {
                    "maximum_relevance" => max_relevance = Some(int(p, "dcg", &field, value)?),
                    "unknown_doc_rating" => default_rating = Some(int(p, "dcg", &field, value)?),
                    "k" => k = Some(int(p, "dcg", &field, value)?),
                    _ => return Err(unknown(field_at, "dcg", &field)),
                }
            }
            let Some(max_relevance) = max_relevance else {
                return Err(DslError::xcontent(
                    p.location(),
                    "[dcg] Required [maximum_relevance]",
                    None,
                ));
            };
            Metric::ExpectedReciprocalRank {
                max_relevance,
                unknown_doc_rating: default_rating,
                k: k.unwrap_or(10),
            }
        }
        _ => {
            return Err(DslError::xcontent(
                at,
                // ALLOC: error path; one message.
                &format!("[rank_eval] unknown field [{name}]"),
                None,
            ));
        }
    };
    if p.next_token() != Some(Kind::EndObject) {
        return Err(DslError::xcontent(
            p.location(),
            "[rank_eval] a metric takes one name",
            None,
        ));
    }
    Ok(result)
}
