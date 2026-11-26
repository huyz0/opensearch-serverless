//! Runs a pass's due shards on at most a given number of scoped threads.

use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `f` over every item, at most `threads` at once on threads that end with
/// the call; the results in item order. One thread (or one item) runs on the
/// calling thread and starts none.
pub(super) fn run_bounded<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let workers = threads.min(items.len());
    if workers <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    // ALLOC: one slot an item of a pass.
    let done: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::SeqCst);
                    let Some(item) = items.get(at) else { break };
                    let result = f(item);
                    done.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((at, result));
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.sort_by_key(|(at, _)| *at);
    done.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // AC-3
    #[test]
    fn no_more_than_the_given_threads_run_at_once_and_results_keep_their_order() {
        let now = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let items: Vec<usize> = (0..10).collect();
        let out = run_bounded(&items, 3, |i| {
            let n = now.fetch_add(1, Ordering::SeqCst).saturating_add(1);
            most.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            now.fetch_sub(1, Ordering::SeqCst);
            *i
        });
        assert_eq!(out, items);
        assert_eq!(most.load(Ordering::SeqCst), 3);
    }

    // AC-3
    #[test]
    fn one_thread_runs_serially_on_the_caller_and_none_for_nothing() {
        let caller = std::thread::current().id();
        let out = run_bounded(&[1, 2, 3], 1, |_| std::thread::current().id());
        assert!(out.iter().all(|id| *id == caller));
        assert!(run_bounded::<u8, u8>(&[], 8, |x| *x).is_empty());
    }
}
