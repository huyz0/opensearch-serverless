//! `rest_total_hits_as_int`: the `total` of a `hits` object written as a number.
//!
//! Ported from `SearchHits.toXContent` (server/src/main/java/org/opensearch/search/SearchHits.java)
//! at OpenSearch 3.10.0, which reads the parameter for every `hits` object of a response: the
//! response's own, each `top_hits` aggregation's and each `inner_hits`'. A scope over one write
//! turns the number on for all of them; it is thread-local because a write runs to its end on one
//! thread and the writers of the nested ones have no other way to reach it without a parameter on
//! each.

use std::cell::Cell;
use std::io::{self, Write};

thread_local! {
    /// Whether the write open on this thread writes totals as numbers.
    static ON: Cell<bool> = const { Cell::new(false) };
}

/// Runs `write` with totals as numbers on or off, restoring what was there before.
pub fn scoped<R>(on: bool, write: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            ON.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(ON.with(|cell| cell.replace(on)));
    write()
}

/// A `hits` object's `"total":..,` of an exact `total`, as a number inside a scope that is on
/// and as `{"value":N,"relation":"eq"}` outside one.
///
/// # Errors
///
/// Any error of `out`.
pub fn write_exact(out: &mut dyn Write, total: u64) -> io::Result<()> {
    if ON.with(Cell::get) {
        write!(out, "\"total\":{total},")
    } else {
        write!(out, "\"total\":{{\"value\":{total},\"relation\":\"eq\"}},")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(total: u64) -> String {
        let mut out = Vec::new();
        write_exact(&mut out, total).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_total_is_an_object_outside_a_scope_and_a_number_inside_one() {
        assert_eq!(written(7), r#""total":{"value":7,"relation":"eq"},"#);
        assert_eq!(scoped(true, || written(7)), r#""total":7,"#);
        assert_eq!(
            scoped(true, || scoped(false, || written(7))),
            r#""total":{"value":7,"relation":"eq"},"#
        );
    }

    #[test]
    fn a_scope_is_restored_after_it_and_after_a_panic() {
        scoped(true, || ());
        assert_eq!(written(1), r#""total":{"value":1,"relation":"eq"},"#);
        let caught = std::panic::catch_unwind(|| scoped(true, || panic!("write failed")));
        assert!(caught.is_err());
        assert_eq!(written(1), r#""total":{"value":1,"relation":"eq"},"#);
    }
}
