//! The overlay's estimate against the heap a build takes, measured by the
//! counting allocator: never below the peak of the build, and at most eight
//! times it or 512 KiB, whichever is larger. Its own binary, because the
//! counting allocator is process-wide.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
use oss_reader::overlay::{Overlay, estimate_bytes};
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

fn mapping() -> Mapping {
    Mapping::parse(
        br#"{"properties":{"name":{"type":"keyword"},"body":{"type":"text"},"n":{"type":"long"},
        "a":{"type":"keyword"},"b":{"type":"keyword"},"c":{"type":"text"},"d":{"type":"long"}}}"#,
    )
    .expect("mapping")
}

/// `count` records whose `body` is `body_bytes` long, with every field set.
fn records(count: usize, body_bytes: usize) -> Vec<WalRecord<'static>> {
    let words = ["alpha", "beta", "gamma", "delta", "kappa", "omega", "sigma"];
    (0..count)
        .map(|i| {
            let mut body = String::new();
            let mut w = i;
            while body.len() < body_bytes {
                body.push_str(words[w % words.len()]);
                body.push(' ');
                w += 3;
            }
            body.truncate(body_bytes);
            let source = format!(
                r#"{{"name":"n{i}","body":"{body}","n":{i},"a":"a{}","b":"b{}","c":"c {i} text","d":{}}}"#,
                i % 7,
                i % 13,
                i * 3
            );
            WalRecord::index(
                format!("id{i}"),
                source,
                i64::try_from(i).expect("fits") + 1,
                1,
                1,
            )
        })
        .collect()
}

#[test]
fn the_estimate_bounds_what_a_build_takes() {
    let m = mapping();
    // The process's first build starts once-only state no overlay holds.
    drop(Overlay::build(&m, &records(5, 50), 0, usize::MAX).expect("warm-up"));
    // Ascending by peak: the allocator's maximum cannot be reset, so a
    // window reads the largest peak of the process so far.
    let shapes = [
        ("one-small", 1, 100),
        ("100-small", 100, 100),
        ("100-1k", 100, 1_000),
        ("1000-small", 1_000, 100),
        ("1000-1k", 1_000, 1_000),
        ("100-64k", 100, 64_000),
        ("10-512k", 10, 512_000),
    ];
    let mut report = Vec::new();
    for (name, count, body) in shapes {
        let recs = records(count, body);
        let estimate = estimate_bytes(&recs);
        let window = PeakWindow::start(&ALLOC);
        let overlay = Overlay::build(&m, &recs, 0, usize::MAX).expect("build");
        let peak = window.peak_bytes();
        let retained = window.live_bytes();
        drop(overlay);
        report.push((name, peak, retained, estimate));
    }
    for (name, peak, retained, estimate) in &report {
        println!("{name}: peak {peak} B, retained {retained} B, estimate {estimate} B");
    }
    for (name, peak, _, estimate) in &report {
        assert!(
            estimate >= peak,
            "{name}: estimate {estimate} under the {peak} B peak: {report:?}"
        );
        assert!(
            *estimate <= (8 * peak).max(512 << 10),
            "{name}: estimate {estimate} over eight times the {peak} B peak: {report:?}"
        );
    }
}
