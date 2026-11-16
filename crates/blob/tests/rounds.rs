//! docs/specs/request-budgets/SPEC.md AC-3: round-trip depth.

use std::sync::{Arc, Barrier};
use std::time::Duration;

use oss_blob::{BlobStore, ByteRange, Emulator, Rounds, SliceSource};

fn store() -> Arc<Rounds<Emulator>> {
    Arc::new(Rounds::new(Emulator::new(1 << 20, 1)))
}

#[test]
fn calls_sent_together_share_a_round() {
    let s = store();
    // Held long, and started together at a barrier, so the four overlap
    // however late a thread is scheduled.
    s.reset(Duration::from_millis(200));
    let start = Barrier::new(4);
    std::thread::scope(|scope| {
        for i in 0..4 {
            let (s, start) = (&s, &start);
            scope.spawn(move || {
                start.wait();
                s.head(&format!("k{i}")).unwrap()
            });
        }
    });
    assert_eq!(s.depth(), 1, "{}", s.detail());
    assert_eq!(s.calls().len(), 4);
    // Two more sent together after those answered: one round deeper.
    let start = Barrier::new(2);
    std::thread::scope(|scope| {
        for i in 0..2 {
            let (s, start) = (&s, &start);
            scope.spawn(move || {
                start.wait();
                s.head(&format!("j{i}")).unwrap()
            });
        }
    });
    assert_eq!(s.depth(), 2, "{}", s.detail());
}

#[test]
fn a_call_after_an_answer_is_the_next_round() {
    let s = store();
    assert_eq!(s.depth(), 0);
    let mut buf = [0u8; 64];
    s.put("a", &mut SliceSource::new(b"x"), &mut buf).unwrap();
    s.head("a").unwrap();
    s.delete("a").unwrap();
    assert_eq!(s.depth(), 3);
    s.reset(Duration::ZERO);
    assert_eq!((s.depth(), s.calls().len()), (0, 0), "a reset starts again");
    s.head("a").unwrap();
    assert_eq!(s.depth(), 1);
    // The meter is the wrapped store's.
    assert_eq!(s.meter().total(), s.inner().meter().total());
    assert_eq!(s.meter().total(), 4);
}

#[test]
fn the_log_names_each_call_and_its_key() {
    let s = store();
    let mut buf = [0u8; 64];
    s.put("p", &mut SliceSource::new(b"body"), &mut buf)
        .unwrap();
    s.reset(Duration::ZERO);
    let mut got = Vec::new();
    let mut sink = |chunk: &[u8]| {
        got.extend_from_slice(chunk);
        Ok(())
    };
    s.get("p", ByteRange::Full, &mut buf, &mut sink).unwrap();
    s.get(
        "p",
        ByteRange::From { start: 1, len: 2 },
        &mut buf,
        &mut sink,
    )
    .unwrap();
    s.create_if_absent("c", &mut SliceSource::new(b"x"), &mut buf)
        .unwrap();
    // LIST: one page, to show a listing is logged.
    s.list_prefix("p", None, 10, &mut |_, _| {}).unwrap();
    s.create_register("r", b"v").unwrap();
    let g = s.read_register("r", &mut buf).unwrap().unwrap().generation;
    s.swap_register("r", g, b"w").unwrap();
    let g = s.read_register("r", &mut buf).unwrap().unwrap().generation;
    assert!(s.delete_register("r", g).unwrap());
    s.validate_key("v").unwrap();
    let kinds: Vec<(&str, String)> = s.calls();
    let expected = [
        ("get", "p"),
        ("get", "p [1+2]"),
        ("create_if_absent", "c"),
        ("list_prefix", "p"),
        ("create_register", "r"),
        ("read_register", "r"),
        ("swap_register", "r"),
        ("read_register", "r"),
        ("delete_register", "r"),
    ];
    assert_eq!(
        kinds,
        expected.map(|(k, v)| (k, v.to_owned())),
        "validate_key sends nothing and is not a call"
    );
    assert_eq!(got, b"bodyod");
    let detail = s.detail();
    assert!(
        detail.starts_with("9 calls, depth 9:\n  get p\n  get p [1+2]\n  create_if_absent c\n"),
        "{detail}"
    );
}
