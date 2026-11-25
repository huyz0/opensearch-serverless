//! Provisional per-call cost of a module (docs/specs/extension-analyzer AC-16, AC-20):
//! run with `cargo test -p oss-ext-wasm --release --test cost -- --ignored --nocapture`.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::{Parts, build};
use oss_ext_wasm::{Host, HostConfig, Unbounded};

#[test]
#[ignore = "a measurement, run by hand"]
fn the_cost_of_one_call_of_the_echo_module() {
    let host = Host::new(HostConfig::default());
    let m = host
        .load(&build(&Parts::default()), Arc::new(Unbounded))
        .expect("loads");
    let input = vec![b'x'; 256];
    for _ in 0..200 {
        m.call("t", 0, &input).expect("warm");
    }
    let mut nanos: Vec<u128> = (0..5000)
        .map(|_| {
            let at = Instant::now();
            m.call("t", 0, &input).expect("call");
            at.elapsed().as_nanos()
        })
        .collect();
    nanos.sort_unstable();
    let at = |q: usize| nanos[nanos.len() * q / 100 - 1] / 1000;
    println!(
        "echo 256 B, 5000 calls: p50 {} us, p99 {} us, max {} us",
        at(50),
        at(99),
        nanos[nanos.len() - 1] / 1000
    );
}
