//! docs/specs/painless-compile-cache AC-1 to AC-5; compiled only under
//! `#[cfg(test)]`.

use std::cell::Cell;

use super::{Cache, Installed};

fn computed(counter: &Cell<u32>, bytes: &[u8]) -> Result<Vec<u8>, String> {
    counter.set(counter.get().saturating_add(1));
    Ok(bytes.to_vec())
}

// AC-1
#[test]
fn a_repeated_key_is_computed_once() {
    let cache = Cache::new(256, 1 << 20);
    let runs = Cell::new(0);
    let a = cache
        .get_or_compute(b"k".as_slice(), || computed(&runs, b"blob"))
        .expect("ok");
    let b = cache
        .get_or_compute(b"k".as_slice(), || computed(&runs, b"other"))
        .expect("ok");
    assert_eq!(&*a, b"blob");
    assert_eq!(&*b, b"blob");
    assert_eq!(runs.get(), 1);
    assert_eq!(cache.counts(), (1, 1));
}

// AC-2
#[test]
fn another_key_gets_its_own_value() {
    let cache = Cache::new(256, 1 << 20);
    let runs = Cell::new(0);
    cache
        .get_or_compute(b"a".as_slice(), || computed(&runs, b"one"))
        .expect("ok");
    let b = cache
        .get_or_compute(b"b".as_slice(), || computed(&runs, b"two"))
        .expect("ok");
    assert_eq!(&*b, b"two");
    assert_eq!(runs.get(), 2);
}

// AC-3
#[test]
fn the_oldest_entry_leaves_at_the_entry_bound() {
    let cache = Cache::new(2, 1 << 20);
    let runs = Cell::new(0);
    for k in ["a", "b", "c"] {
        cache
            .get_or_compute(k.as_bytes(), || computed(&runs, b"v"))
            .expect("ok");
    }
    assert_eq!(cache.len(), 2);
    cache
        .get_or_compute(b"c".as_slice(), || computed(&runs, b"v"))
        .expect("ok");
    assert_eq!(runs.get(), 3, "c is still held");
    cache
        .get_or_compute(b"a".as_slice(), || computed(&runs, b"v"))
        .expect("ok");
    assert_eq!(runs.get(), 4, "a left first");
}

// AC-3
#[test]
fn charged_bytes_never_pass_the_budget_and_a_value_over_it_is_not_kept() {
    let cache = Cache::new(256, 100);
    let runs = Cell::new(0);
    for i in 0..20 {
        let key = format!("key{i}");
        cache
            .get_or_compute(key.as_bytes(), || computed(&runs, &[0u8; 30]))
            .expect("ok");
        assert!(cache.bytes() <= 100, "{} bytes", cache.bytes());
    }
    let big = cache
        .get_or_compute(b"big".as_slice(), || computed(&runs, &[1u8; 500]))
        .expect("served");
    assert_eq!(big.len(), 500);
    assert!(cache.bytes() <= 100);
    let before = runs.get();
    cache
        .get_or_compute(b"big".as_slice(), || computed(&runs, &[1u8; 500]))
        .expect("ok");
    assert_eq!(
        runs.get(),
        before.saturating_add(1),
        "a value over the budget is computed each time"
    );
}

// AC-5
#[test]
fn an_error_is_not_cached() {
    let cache = Cache::new(256, 1 << 20);
    let runs = Cell::new(0);
    for _ in 0..2 {
        let e = cache.get_or_compute(b"bad".as_slice(), || {
            runs.set(runs.get() + 1);
            Err::<Vec<u8>, _>("compile error".to_owned())
        });
        assert_eq!(e.expect_err("refused"), "compile error");
    }
    assert_eq!(runs.get(), 2);
    assert_eq!(cache.len(), 0);
}

// AC-4
#[test]
fn removing_the_cache_drops_what_paid_for_it() {
    struct Payment(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Payment {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    drop(Installed::new(Box::new(Payment(released.clone())), 1 << 20));
    assert!(released.load(std::sync::atomic::Ordering::SeqCst));
}

mod encode {
    use crate::dsl::queries::function_score_painless::encode;
    use crate::dsl::queries::function_score_script::{Lang, Param, ScriptScore};
    use crate::mapping::Mapping;

    fn script(source: &str, factor: i32) -> ScriptScore {
        ScriptScore {
            source: source.to_owned(),
            lang: Lang::Painless,
            params: vec![("factor".to_owned(), Param::Int(factor))],
        }
    }

    fn mapping(ty: &str) -> Mapping {
        Mapping::from_json(&format!(r#"{{"properties":{{"year":{{"type":"{ty}"}}}}}}"#))
            .expect("mapping")
    }

    // AC-1, AC-2 and AC-5: one test, because the cache is process-wide and
    // tests of the library run in parallel.
    #[test]
    fn a_cached_blob_is_the_blob_and_no_input_is_missed() {
        crate::painless_cache::install(Box::new(()));
        let long = mapping("long");
        let s = script("doc['year'].value * params.factor", 2);
        let cold = encode(&long, &s, 0).expect("encodes");
        let held = crate::painless_cache::bytes();
        assert!(held > 0, "nothing was kept");
        assert_eq!(encode(&long, &s, 0).expect("again"), cold);
        assert_eq!(
            crate::painless_cache::bytes(),
            held,
            "the repeat added entries"
        );
        // A param, a field's kind and the context each give their own blob.
        let other_param =
            encode(&long, &script("doc['year'].value * params.factor", 3), 0).expect("encodes");
        assert_ne!(other_param, cold);
        let other_kind = encode(&mapping("double"), &s, 0).expect("encodes");
        assert_ne!(other_kind, cold);
        let other_context = encode(&long, &s, 3).expect("encodes");
        assert_ne!(other_context, cold);
        // The cached ones still answer as they did.
        assert_eq!(encode(&long, &s, 0).expect("again"), cold);
        assert_eq!(
            encode(&mapping("double"), &s, 0).expect("again"),
            other_kind
        );
        // A refusal is the same each time and keeps nothing.
        let bad = script("int x = 'a'; return x;", 1);
        let before = crate::painless_cache::bytes();
        let first = encode(&long, &bad, 0).expect_err("refused").reason;
        assert_eq!(encode(&long, &bad, 0).expect_err("refused").reason, first);
        assert_eq!(crate::painless_cache::bytes(), before);
        crate::painless_cache::remove();
        assert_eq!(crate::painless_cache::bytes(), 0);
        assert_eq!(encode(&long, &s, 0).expect("uncached"), cold);
    }
}
