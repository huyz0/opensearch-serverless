//! The evaluation metrics of `_rank_eval`: `PrecisionAtK`, `RecallAtK` and
//! `MeanReciprocalRank` (modules/rank-eval/src/main/java/org/opensearch/index/rankeval/)
//! at OpenSearch 3.10.0, over hits already joined with their ratings.

use std::collections::HashMap;

/// A search hit as a metric sees it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocKey {
    /// The hit's index.
    pub index: String,
    /// The hit's id.
    pub id: String,
}

/// The metric of a request and its parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// `precision`.
    Precision {
        /// The lowest rating that counts as relevant.
        threshold: i32,
        /// Whether hits with no rating are left out of the count.
        ignore_unlabeled: bool,
        /// The window.
        k: i32,
    },
    /// `recall`.
    Recall {
        /// The lowest rating that counts as relevant.
        threshold: i32,
        /// The window.
        k: i32,
    },
    /// `mean_reciprocal_rank`.
    ReciprocalRank {
        /// The lowest rating that counts as relevant.
        threshold: i32,
        /// The window.
        k: i32,
    },
}

/// What a metric reports beside its score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// `precision`.
    Precision {
        /// Relevant hits retrieved.
        relevant_retrieved: i64,
        /// Hits retrieved that count.
        retrieved: i64,
    },
    /// `recall`.
    Recall {
        /// Relevant hits retrieved.
        relevant_retrieved: i64,
        /// Relevant rated documents.
        relevant: i64,
    },
    /// `mean_reciprocal_rank`.
    ReciprocalRank {
        /// The rank of the first relevant hit, or -1.
        first_relevant: i64,
    },
}

/// One request evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct Quality {
    /// The score.
    pub score: f64,
    /// The metric's details.
    pub detail: Detail,
    /// The rating of each hit, in hit order.
    pub ratings: Vec<Option<i32>>,
}

impl Metric {
    /// The window: the size every rated search is forced to.
    pub fn k(self) -> i32 {
        match self {
            Metric::Precision { k, .. }
            | Metric::Recall { k, .. }
            | Metric::ReciprocalRank { k, .. } => k,
        }
    }

    /// `evaluate`: the score of `hits` against `rated`.
    pub fn evaluate(self, hits: &[DocKey], rated: &[(DocKey, i32)]) -> Quality {
        // ALLOC: one entry a rated document of the request, which the body bounds.
        let by_key: HashMap<&DocKey, i32> = rated.iter().map(|(k, r)| (k, *r)).collect();
        // ALLOC: one entry a hit, at most the window.
        let ratings: Vec<Option<i32>> = hits.iter().map(|h| by_key.get(h).copied()).collect();
        let (score, detail) = match self {
            Metric::Precision {
                threshold,
                ignore_unlabeled,
                ..
            } => {
                let mut relevant_retrieved = 0_i64;
                let mut retrieved = 0_i64;
                for rating in &ratings {
                    match rating {
                        Some(r) => {
                            retrieved = retrieved.saturating_add(1);
                            if *r >= threshold {
                                relevant_retrieved = relevant_retrieved.saturating_add(1);
                            }
                        }
                        None if !ignore_unlabeled => retrieved = retrieved.saturating_add(1),
                        None => {}
                    }
                }
                let score = if retrieved > 0 {
                    ratio(relevant_retrieved, retrieved)
                } else {
                    0.0
                };
                (
                    score,
                    Detail::Precision {
                        relevant_retrieved,
                        retrieved,
                    },
                )
            }
            Metric::Recall { threshold, .. } => {
                let relevant_retrieved = count(ratings.iter().flatten(), threshold);
                let relevant = count(rated.iter().map(|(_, r)| r), threshold);
                let score = if relevant > 0 {
                    ratio(relevant_retrieved, relevant)
                } else {
                    0.0
                };
                (
                    score,
                    Detail::Recall {
                        relevant_retrieved,
                        relevant,
                    },
                )
            }
            Metric::ReciprocalRank { threshold, .. } => {
                let first = ratings
                    .iter()
                    .position(|r| r.is_some_and(|r| r >= threshold))
                    .and_then(|i| i64::try_from(i.saturating_add(1)).ok())
                    .unwrap_or(-1);
                let score = if first == -1 { 0.0 } else { ratio(1, first) };
                (
                    score,
                    Detail::ReciprocalRank {
                        first_relevant: first,
                    },
                )
            }
        };
        Quality {
            score,
            detail,
            ratings,
        }
    }
}

/// How many of `ratings` reach `threshold`.
fn count<'a>(ratings: impl Iterator<Item = &'a i32>, threshold: i32) -> i64 {
    ratings.fold(0_i64, |n, r| {
        if *r >= threshold {
            n.saturating_add(1)
        } else {
            n
        }
    })
}

/// `a / b` as Java divides two numbers into a double.
// ARITH: both are counts far below 2^53, and b is not zero.
#[allow(clippy::cast_precision_loss, clippy::arithmetic_side_effects)]
fn ratio(a: i64, b: i64) -> f64 {
    // ARITH: both are counts far below 2^53, and b is not zero.
    a as f64 / b as f64
}

/// `combine`: the mean of the scores.
// ARITH: scores is not empty, and its length is far below 2^53.
#[allow(clippy::cast_precision_loss, clippy::arithmetic_side_effects)]
pub fn combine(scores: &[f64]) -> f64 {
    // ARITH: scores is not empty, and its length is far below 2^53.
    scores.iter().sum::<f64>() / scores.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: &str) -> DocKey {
        DocKey {
            index: "t".into(),
            id: i.into(),
        }
    }

    fn rated(list: &[(&str, i32)]) -> Vec<(DocKey, i32)> {
        list.iter().map(|(i, r)| (key(i), *r)).collect()
    }

    #[test]
    fn precision_counts_unlabeled_hits_unless_ignored() {
        let hits = [key("a"), key("b"), key("c")];
        let ratings = rated(&[("a", 1), ("b", 0)]);
        let p = |ignore| Metric::Precision {
            threshold: 1,
            ignore_unlabeled: ignore,
            k: 10,
        };
        let q = p(false).evaluate(&hits, &ratings);
        assert_eq!(q.score, 1.0 / 3.0);
        assert_eq!(
            q.detail,
            Detail::Precision {
                relevant_retrieved: 1,
                retrieved: 3
            }
        );
        assert_eq!(q.ratings, [Some(1), Some(0), None]);
        assert_eq!(p(true).evaluate(&hits, &ratings).score, 0.5);
        assert_eq!(p(true).evaluate(&[], &ratings).score, 0.0);
    }

    #[test]
    fn recall_divides_by_every_relevant_rated_document() {
        let m = Metric::Recall {
            threshold: 2,
            k: 10,
        };
        let ratings = rated(&[("a", 2), ("b", 3), ("c", 1), ("d", 2)]);
        let q = m.evaluate(&[key("a"), key("x"), key("c")], &ratings);
        assert_eq!(q.score, 1.0 / 3.0);
        assert_eq!(
            q.detail,
            Detail::Recall {
                relevant_retrieved: 1,
                relevant: 3
            }
        );
        assert_eq!(m.evaluate(&[key("a")], &rated(&[("a", 1)])).score, 0.0);
    }

    #[test]
    fn reciprocal_rank_is_one_over_the_first_relevant_rank() {
        let m = Metric::ReciprocalRank {
            threshold: 1,
            k: 10,
        };
        let ratings = rated(&[("c", 1)]);
        let q = m.evaluate(&[key("a"), key("b"), key("c")], &ratings);
        assert_eq!(q.score, 1.0 / 3.0);
        assert_eq!(q.detail, Detail::ReciprocalRank { first_relevant: 3 });
        let q = m.evaluate(&[key("a")], &ratings);
        assert_eq!(q.score, 0.0);
        assert_eq!(q.detail, Detail::ReciprocalRank { first_relevant: -1 });
    }

    #[test]
    fn combine_is_the_mean() {
        assert_eq!(combine(&[1.0, 0.5, 0.0]), 0.5);
    }
}
