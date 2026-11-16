//! The named pools a node budget is split into.

use std::fmt;

/// A named share of the node memory budget.
///
/// Each pool has its own byte limit, so one kind of work (a large bulk body,
/// a wide aggregation) cannot take memory from another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pool {
    /// REST request and response bodies in flight.
    RequestBodies,
    /// Query execution: collectors, aggregations, sort buffers.
    QueryExecution,
    /// Documents buffered before they reach the node write log.
    WriteBuffers,
    /// Cached segment pages read from the object store.
    PageCache,
    /// Node-to-node transport buffers.
    Transport,
    /// What open indexes keep in the heap between searches.
    OpenIndexes,
}

impl Pool {
    /// Every pool, in a fixed order.
    pub const ALL: [Pool; 6] = [
        Pool::RequestBodies,
        Pool::QueryExecution,
        Pool::WriteBuffers,
        Pool::PageCache,
        Pool::Transport,
        Pool::OpenIndexes,
    ];

    /// The pool's position in [`Pool::ALL`].
    pub(crate) const fn index(self) -> usize {
        match self {
            Pool::RequestBodies => 0,
            Pool::QueryExecution => 1,
            Pool::WriteBuffers => 2,
            Pool::PageCache => 3,
            Pool::Transport => 4,
            Pool::OpenIndexes => 5,
        }
    }

    /// The pool's name in settings and error messages.
    pub const fn name(self) -> &'static str {
        match self {
            Pool::RequestBodies => "request_bodies",
            Pool::QueryExecution => "query_execution",
            Pool::WriteBuffers => "write_buffers",
            Pool::PageCache => "page_cache",
            Pool::Transport => "transport",
            Pool::OpenIndexes => "open_indexes",
        }
    }
}

impl fmt::Display for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_follow_all_order() {
        for (i, pool) in Pool::ALL.into_iter().enumerate() {
            assert_eq!(pool.index(), i);
        }
    }

    #[test]
    fn names_are_distinct_and_display_matches() {
        let names: Vec<&str> = Pool::ALL.iter().map(|p| p.name()).collect();
        for (i, a) in names.iter().enumerate() {
            for b in names.iter().skip(i.saturating_add(1)) {
                assert_ne!(a, b);
            }
        }
        assert_eq!(Pool::PageCache.to_string(), "page_cache");
    }
}
