//! Running one blob read for each of a list of items, at most `width` at a
//! time, answers in item order.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

type Slot<T> = Mutex<Option<Option<T>>>;

/// Calls `call` on each of `items` from at most `width` threads (the caller
/// is one of them) and returns the answers in item order; `None` for a call
/// that panicked. Each helper counts its requests to the caller's request.
pub(crate) fn fan_out<I, T, F>(width: usize, items: &[I], call: F) -> Vec<Option<T>>
where
    I: Sync,
    T: Send,
    F: Fn(&I) -> T + Sync,
{
    // ALLOC: one slot per item, bounded by the request's shards.
    let slots: Vec<Slot<T>> = items.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let helpers = width.max(1).min(items.len()).saturating_sub(1);
    let request = oss_blob::cost::capture();
    std::thread::scope(|scope| {
        for _ in 0..helpers {
            scope.spawn(|| {
                let _scope = oss_blob::cost::enter(request.clone());
                drain(&next, items, &slots, &call);
            });
        }
        drain(&next, items, &slots, &call);
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .flatten()
        })
        // ALLOC: one answer per item.
        .collect()
}

fn drain<I, T, F: Fn(&I) -> T>(next: &AtomicUsize, items: &[I], slots: &[Slot<T>], call: &F) {
    loop {
        let at = next.fetch_add(1, Ordering::Relaxed);
        let (Some(item), Some(slot)) = (items.get(at), slots.get(at)) else {
            return;
        };
        let answer = catch_unwind(AssertUnwindSafe(|| call(item))).ok();
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(answer);
    }
}

#[cfg(test)]
mod tests {
    use super::fan_out;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn answers_come_back_in_item_order_at_any_width() {
        let items: Vec<u32> = (0..50).collect();
        for width in [0, 1, 3, 8, 100] {
            let out = fan_out(width, &items, |n| n * 2);
            assert_eq!(out, items.iter().map(|n| Some(n * 2)).collect::<Vec<_>>());
        }
        assert!(fan_out(4, &[] as &[u32], |n| *n).is_empty());
    }

    #[test]
    fn no_more_than_width_run_at_once_and_that_many_do() {
        let (now, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let items = [0u8; 16];
        fan_out(4, &items, |_| {
            let n = now.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(30));
            now.fetch_sub(1, Ordering::SeqCst);
        });
        assert_eq!(peak.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn a_panic_is_one_missing_answer() {
        let out = fan_out(2, &[1u32, 2, 3], |n| {
            assert!(*n != 2, "boom");
            *n
        });
        assert_eq!(out, vec![Some(1), None, Some(3)]);
    }
}
