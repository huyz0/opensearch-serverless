//! Merges the hits of every shard into one page: the totals, the best score,
//! the flags, and the ordered, windowed merge of the hits.
//!
//! The merge keeps at most twice the window (`from + size`) of hits at any
//! moment: once the pile is that deep it is sorted and cut back to the
//! window, so a wide index costs the coordinator two windows of hits rather
//! than a window a shard.

use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt;

use oss_reader::search::SortValue;

/// `track_total_hits_up_to`'s accurate bound: count every match.
pub const ACCURATE: i64 = i32::MAX as i64;

/// `track_total_hits`' default bound.
pub const DEFAULT_TRACK_TOTAL_HITS_UP_TO: i64 = 10_000;

/// How one sort key runs, from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    /// Whether larger values come first.
    pub descending: bool,
    /// Whether a missing value goes first (the sort's `_first`), whichever
    /// the direction; otherwise it goes last.
    pub missing_first: bool,
}

/// One shard's hit: what it is ranked by and what the caller renders from.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardHit<P> {
    /// The score; `None` when the hit has none.
    pub score: Option<f32>,
    /// The sort values; empty when sorted by score.
    pub sort: Vec<SortValue>,
    /// What the caller carries to render the hit.
    pub payload: P,
}

/// One shard's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardAnswer<P> {
    /// How many documents matched, exactly when `exact`.
    pub total: u64,
    /// Whether `total` is exact rather than a lower bound.
    pub exact: bool,
    /// The best score the shard had.
    pub max_score: Option<f32>,
    /// The shard's hits, at most the window.
    pub hits: Vec<ShardHit<P>>,
    /// Whether the shard timed out.
    pub timed_out: bool,
    /// Whether the shard stopped early, when it says.
    pub terminated_early: Option<bool>,
}

/// Whether a total is the count or a lower bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The count.
    Equal,
    /// At least this many.
    AtLeast,
}

/// Why shards' hits cannot be merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeError {
    /// Two hits' sort values at one key are of different kinds.
    IncompatibleSortValues,
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompatibleSortValues => f.write_str(
                "cannot merge incompatible sort values across indices; use consistent field \
                 mappings or numeric_type for numeric sorts",
            ),
        }
    }
}

impl std::error::Error for MergeError {}

/// The merged answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Merged<P> {
    /// The total reported for the tracked bound.
    pub total: u64,
    /// Whether `total` is the count or a lower bound.
    pub relation: Relation,
    /// The best score any shard had.
    pub max_score: Option<f32>,
    /// Whether any shard timed out.
    pub timed_out: bool,
    /// Whether any shard terminated early; unset until one says.
    pub terminated_early: Option<bool>,
    /// How many shards answered.
    pub answered: usize,
    /// The hits from `from`, at most `size`.
    pub page: Vec<ShardHit<P>>,
}

/// Accumulates shard answers into one page.
pub struct Merger<P> {
    from: usize,
    size: usize,
    sorts: Vec<SortSpec>,
    hits: Vec<ShardHit<P>>,
    total: u64,
    relation: Relation,
    max_score: Option<f32>,
    timed_out: bool,
    terminated_early: Option<bool>,
    answered: usize,
    peak: usize,
    incompatible: bool,
}

fn score_of(hit: &impl HasScore) -> f32 {
    hit.score()
        .filter(|s| !s.is_nan())
        .unwrap_or(f32::NEG_INFINITY)
}

trait HasScore {
    fn score(&self) -> Option<f32>;
}

impl<P> HasScore for ShardHit<P> {
    fn score(&self) -> Option<f32> {
        self.score
    }
}

fn is_null(v: &SortValue) -> bool {
    matches!(v, SortValue::Term(None))
}

/// `compareOne`: values of one kind compare; of two kinds they cannot.
fn compare_one(a: &SortValue, b: &SortValue) -> Option<Ordering> {
    match (a, b) {
        (SortValue::Long(x), SortValue::Long(y)) => Some(x.cmp(y)),
        (SortValue::Term(Some(x)), SortValue::Term(Some(y))) => Some(x.cmp(y)),
        (SortValue::Score(x), SortValue::Score(y)) => Some(x.total_cmp(y)),
        (SortValue::Double(x), SortValue::Double(y)) => Some(x.total_cmp(y)),
        (SortValue::Float(x), SortValue::Float(y)) => Some(x.total_cmp(y)),
        _ => None,
    }
}

impl<P> Merger<P> {
    /// A merge of pages at `from` and `size`, ordered by `sorts`, or by score
    /// without any.
    pub fn new(from: usize, size: usize, sorts: Option<Vec<SortSpec>>) -> Self {
        Self {
            from,
            size,
            sorts: sorts.unwrap_or_default(),
            // ALLOC: at most twice the window plus one shard's answer.
            hits: Vec::new(),
            total: 0,
            relation: Relation::Equal,
            max_score: None,
            timed_out: false,
            terminated_early: None,
            answered: 0,
            peak: 0,
            incompatible: false,
        }
    }

    fn window(&self) -> usize {
        self.from.saturating_add(self.size).max(1)
    }

    /// How many hits the pile holds now.
    pub fn held(&self) -> usize {
        self.hits.len()
    }

    /// The most hits the pile held at once, for the memory bound.
    pub fn peak_held(&self) -> usize {
        self.peak
    }

    fn compare(&self, a: &ShardHit<P>, b: &ShardHit<P>, bad: &Cell<bool>) -> Ordering {
        if self.sorts.is_empty() {
            // Descending by score; an unscored hit sorts last rather than
            // taking the top of the page from one that scored.
            return score_of(b).total_cmp(&score_of(a));
        }
        match (a.sort.is_empty(), b.sort.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (i, (left, right)) in a.sort.iter().zip(&b.sort).enumerate() {
            let (left_null, right_null) = (is_null(left), is_null(right));
            if left_null || right_null {
                if left_null && right_null {
                    continue;
                }
                // Placed by the sort's own missing rule, not by direction.
                let first = self.sorts.get(i).is_some_and(|s| s.missing_first);
                return if left_null == first {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            let Some(ordering) = compare_one(left, right) else {
                bad.set(true);
                return Ordering::Equal;
            };
            if ordering != Ordering::Equal {
                let descending = self.sorts.get(i).is_some_and(|s| s.descending);
                return if descending {
                    ordering.reverse()
                } else {
                    ordering
                };
            }
        }
        Ordering::Equal
    }

    /// Sorts the pile, stable in the order the shards answered in.
    fn sort(&mut self) -> Result<(), MergeError> {
        let bad = Cell::new(false);
        let mut hits = std::mem::take(&mut self.hits);
        hits.sort_by(|a, b| self.compare(a, b, &bad));
        self.hits = hits;
        if bad.get() {
            self.incompatible = true;
        }
        if self.incompatible {
            return Err(MergeError::IncompatibleSortValues);
        }
        Ok(())
    }

    /// Adds one shard's answer.
    ///
    /// # Errors
    ///
    /// [`MergeError::IncompatibleSortValues`] when trimming the pile compared
    /// sort values of different kinds.
    pub fn absorb(&mut self, answer: ShardAnswer<P>) -> Result<(), MergeError> {
        self.total = self.total.saturating_add(answer.total);
        if !answer.exact {
            self.relation = Relation::AtLeast;
        }
        if let Some(score) = answer.max_score.filter(|s| !s.is_nan()) {
            self.max_score = Some(self.max_score.map_or(score, |best| best.max(score)));
        }
        self.timed_out |= answer.timed_out;
        if let Some(early) = answer.terminated_early {
            self.terminated_early = Some(self.terminated_early.unwrap_or(false) || early);
        }
        self.answered = self.answered.saturating_add(1);
        self.hits.extend(answer.hits);
        self.peak = self.peak.max(self.hits.len());
        let window = self.window();
        if self.hits.len() > window.saturating_mul(2) {
            self.sort()?;
            self.hits.truncate(window);
        }
        Ok(())
    }

    /// The merged answer, with the total reported for `track_total_hits_up_to`
    /// (the default bound when unset).
    ///
    /// # Errors
    ///
    /// [`MergeError::IncompatibleSortValues`] as [`Self::absorb`].
    pub fn finish(mut self, track_total_hits_up_to: Option<i64>) -> Result<Merged<P>, MergeError> {
        self.sort()?;
        let page = self
            .hits
            .into_iter()
            .skip(self.from)
            .take(self.size)
            // ALLOC: the page, at most size hits.
            .collect();
        Ok(Merged {
            total: ceiling(self.total, self.relation, track_total_hits_up_to),
            relation: self.relation,
            max_score: self.max_score,
            timed_out: self.timed_out,
            terminated_early: self.terminated_early,
            answered: self.answered,
            page,
        })
    }
}

/// A total that is only a lower bound is reported as at most the tracked
/// bound; a bound of zero or below, or the accurate one, reports the count.
fn ceiling(total: u64, relation: Relation, up_to: Option<i64>) -> u64 {
    if relation != Relation::AtLeast {
        return total;
    }
    let bound = up_to.unwrap_or(DEFAULT_TRACK_TOTAL_HITS_UP_TO);
    if bound <= 0 || bound == ACCURATE {
        return total;
    }
    total.min(u64::try_from(bound).unwrap_or(u64::MAX))
}
