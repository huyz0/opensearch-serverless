//! The running bucket total of one reduce.
//!
//! OpenSearch counts every bucket a reduce makes, at every level of a nested
//! aggregation, against `search.max_buckets`. A reduce here builds a level a
//! parent bucket at a time, so a count after the fact would come too late: a
//! gap-filled `histogram` under each of 65,535 parent buckets is made before
//! any total is known. [`scoped`] opens a total for one reduce, each level asks
//! [`remaining`] before it fills and calls [`add`] for what it makes, and the
//! reduce's caller reads the excess from [`scoped`]'s result. Outside a scope
//! nothing is counted and the whole limit remains, as for a shard's own pass.
//!
//! The total is thread-local: a reduce runs to its end on the thread that
//! started it, and a level has no other way to reach the reduce's total
//! without a parameter on every reducer.

use std::cell::Cell;

use crate::aggs::bucket::BUCKET_LIMIT;

thread_local! {
    /// Scopes open on this thread (a nested scope joins the outer one).
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    /// The buckets counted by the outermost open scope.
    static MADE: Cell<usize> = const { Cell::new(0) };
}

/// How many more buckets this reduce may make: the whole limit outside a
/// scope.
pub fn remaining() -> usize {
    if DEPTH.with(Cell::get) == 0 {
        return BUCKET_LIMIT;
    }
    BUCKET_LIMIT.saturating_sub(MADE.with(Cell::get))
}

/// Counts `n` more buckets; `false` once the total passes the limit. Outside
/// a scope nothing is counted.
pub fn add(n: usize) -> bool {
    if DEPTH.with(Cell::get) == 0 {
        return true;
    }
    let made = MADE.with(|m| {
        let next = m.get().saturating_add(n);
        m.set(next);
        next
    });
    made <= BUCKET_LIMIT
}

/// Runs `f` with a total open: `f`'s result, and the count reached if it
/// passed the limit. A scope inside another one adds to the outer total and
/// reports nothing itself.
pub fn scoped<T>(f: impl FnOnce() -> T) -> (T, Option<usize>) {
    let outer = DEPTH.with(|d| {
        let was = d.get();
        d.set(was.saturating_add(1));
        was
    });
    if outer == 0 {
        MADE.with(|m| m.set(0));
    }
    let out = f();
    DEPTH.with(|d| d.set(outer));
    let made = MADE.with(Cell::get);
    if outer != 0 {
        return (out, None);
    }
    MADE.with(|m| m.set(0));
    (out, (made > BUCKET_LIMIT).then_some(made))
}
