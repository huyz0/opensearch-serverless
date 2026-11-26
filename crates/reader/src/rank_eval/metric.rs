//! The evaluation metrics of `_rank_eval`: `PrecisionAtK`, `RecallAtK`,
//! `MeanReciprocalRank`, `DiscountedCumulativeGain` and `ExpectedReciprocalRank` (modules/rank-eval/src/main/java/org/opensearch/index/rankeval/)
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
    /// `dcg`.
    Dcg {
        /// Whether the score is divided by the ideal gain (`ndcg`).
        normalize: bool,
        /// The rating a hit with none counts as; without one it adds nothing.
        unknown_doc_rating: Option<i32>,
        /// The window.
        k: i32,
    },
    /// `expected_reciprocal_rank`.
    ExpectedReciprocalRank {
        /// The highest rating expected in the data.
        max_relevance: i32,
        /// The rating a hit with none counts as; without one it is skipped.
        unknown_doc_rating: Option<i32>,
        /// The window.
        k: i32,
    },
}

/// What a metric reports beside its score.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// `dcg`.
    Dcg {
        /// The discounted cumulative gain of the hits.
        dcg: f64,
        /// The gain of the best order of the rated documents; 0 unless normalised.
        ideal_dcg: f64,
        /// Hits with no rating.
        unrated: i64,
    },
    /// `expected_reciprocal_rank`.
    ExpectedReciprocalRank {
        /// Hits with no rating.
        unrated: i64,
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
            | Metric::ReciprocalRank { k, .. }
            | Metric::Dcg { k, .. }
            | Metric::ExpectedReciprocalRank { k, .. } => k,
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
            Metric::ExpectedReciprocalRank {
                max_relevance,
                unknown_doc_rating,
                k,
            } => {
                // The Java cuts the hits to the window itself.
                let window = usize::try_from(k).unwrap_or(0);
                let ratings = ratings.get(..window).unwrap_or(&ratings);
                let two_pow_max = 2.0_f64.powi(max_relevance);
                let mut p = 1.0_f64;
                let mut err = 0.0_f64;
                for (at, rating) in ratings.iter().enumerate() {
                    if let Some(r) = rating.or(unknown_doc_rating) {
                        let prob = (2.0_f64.powi(r) - 1.0) / two_pow_max;
                        // ARITH: ranks are far below 2^53.
                        #[allow(clippy::cast_precision_loss, clippy::arithmetic_side_effects)]
                        let rank = (at + 1) as f64;
                        // ARITH: doubles.
                        #[allow(clippy::arithmetic_side_effects)]
                        {
                            err += p * prob / rank;
                            p *= 1.0 - prob;
                        }
                    }
                }
                let unrated =
                    i64::try_from(ratings.iter().filter(|r| r.is_none()).count()).unwrap_or(0);
                (err, Detail::ExpectedReciprocalRank { unrated })
            }
            Metric::Dcg {
                normalize,
                unknown_doc_rating,
                ..
            } => {
                // ALLOC: one entry a hit, at most the window.
                let in_hits: Vec<Option<i32>> =
                    ratings.iter().map(|r| r.or(unknown_doc_rating)).collect();
                let unrated =
                    i64::try_from(ratings.iter().filter(|r| r.is_none()).count()).unwrap_or(0);
                let dcg = discounted_gain(&in_hits);
                let (mut score, mut ideal_dcg) = (dcg, 0.0);
                if normalize {
                    // ALLOC: one entry a rated document of the request, which the body bounds.
                    let mut all: Vec<i32> = rated.iter().map(|(_, r)| *r).collect();
                    all.sort_unstable_by(|a, b| b.cmp(a));
                    let window = in_hits.len().min(all.len());
                    // ALLOC: one entry a hit, at most the window.
                    let best: Vec<Option<i32>> =
                        all.iter().take(window).map(|r| Some(*r)).collect();
                    ideal_dcg = discounted_gain(&best);
                    score = if ideal_dcg == 0.0 {
                        0.0
                    } else {
                        dcg / ideal_dcg
                    };
                }
                (
                    score,
                    Detail::Dcg {
                        dcg,
                        ideal_dcg,
                        unrated,
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

/// `computeDCG`: the sum of `(2^rating - 1) / log2(rank + 1)`, a hit with no
/// rating adding nothing but keeping its rank.
// ARITH: ranks are far below 2^53.
#[allow(clippy::cast_precision_loss, clippy::arithmetic_side_effects)]
fn discounted_gain(ratings: &[Option<i32>]) -> f64 {
    let log2 = 2.0_f64.ln();
    let mut dcg = 0.0;
    for (at, rating) in ratings.iter().enumerate() {
        if let Some(r) = rating {
            // ARITH: both are small and exact in a double.
            dcg += (2.0_f64.powi(*r) - 1.0) / (((at + 2) as f64).ln() / log2);
        }
    }
    dcg
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
    fn dcg_discounts_each_rank_and_ndcg_divides_by_the_ideal_order() {
        let m = |normalize, unknown_doc_rating| Metric::Dcg {
            normalize,
            unknown_doc_rating,
            k: 10,
        };
        let ratings = rated(&[("a", 3), ("b", 2), ("c", 3), ("d", 0), ("e", 1), ("f", 2)]);
        let hits = [key("a"), key("b"), key("c"), key("d"), key("e"), key("f")];
        // The worked example of the Java's tests: 13.84826362927298.
        let q = m(false, None).evaluate(&hits, &ratings);
        assert!(
            (q.score - 13.848_263_629_272_98).abs() < 1e-12,
            "{}",
            q.score
        );
        let q = m(true, None).evaluate(&hits, &ratings);
        let ideal = 14.595_390_756_454_922;
        assert!((q.score - 13.848_263_629_272_98 / ideal).abs() < 1e-12);
        // An unrated hit keeps its rank and adds nothing, or its default.
        let hits = [key("a"), key("x"), key("c")];
        let plain = m(false, None).evaluate(&hits, &ratings);
        let with = m(false, Some(1)).evaluate(&hits, &ratings);
        assert!(with.score > plain.score);
        match plain.detail {
            Detail::Dcg { unrated, .. } => assert_eq!(unrated, 1),
            other => panic!("{other:?}"),
        }
        // Nothing rated at all has no ideal order: ndcg is 0.
        assert_eq!(m(true, None).evaluate(&hits, &[]).score, 0.0);
    }

    #[test]
    fn err_weighs_each_rank_by_the_chance_no_earlier_hit_satisfied() {
        let m = |unknown_doc_rating, k| Metric::ExpectedReciprocalRank {
            max_relevance: 3,
            unknown_doc_rating,
            k,
        };
        // The Java tests' worked examples, ratings 3, 2, 0, 1.
        let ratings = rated(&[("a", 3), ("b", 2), ("c", 0), ("d", 1)]);
        let hits = [key("a"), key("b"), key("c"), key("d")];
        assert_eq!(m(Some(0), 3).evaluate(&hits, &ratings).score, 0.8984375);
        assert_eq!(
            m(Some(0), 4).evaluate(&hits, &ratings).score,
            0.8984375 + 0.00244140625
        );
        // The second hit unrated: skipped, or counted as the default.
        let ratings = rated(&[("a", 3), ("c", 0), ("d", 1)]);
        let hits = [key("a"), key("x"), key("c"), key("d")];
        let q = m(None, 4).evaluate(&hits, &ratings);
        assert_eq!(q.score, 0.875 + 0.00390625);
        assert_eq!(q.detail, Detail::ExpectedReciprocalRank { unrated: 1 });
        assert_eq!(
            m(Some(2), 4).evaluate(&hits, &ratings).score,
            0.8984375 + 0.00244140625
        );
        assert_eq!(m(Some(0), 10).evaluate(&[], &ratings).score, 0.0);
    }

    #[test]
    fn combine_is_the_mean() {
        assert_eq!(combine(&[1.0, 0.5, 0.0]), 0.5);
    }
}
