//! docs/specs/extension-analyzer AC-17: what each tenant's calls cost is
//! counted exactly.

mod common;

use std::sync::Arc;

use common::{IO, Parts, build, with_body};
use oss_ext_wasm::{Host, HostConfig, Module, Unbounded};

fn load(host: &Host, wasm: &[u8]) -> Module {
    host.load(wasm, Arc::new(Unbounded)).expect("loads")
}

fn host() -> Host {
    Host::new(HostConfig::default())
}

// docs/specs/extension-analyzer AC-17: the counters are exact over a counted run
#[test]
fn the_meter_counts_each_tenants_calls_bytes_and_fuel() {
    let host = host();
    let m = load(&host, &build(&Parts::default()));
    for _ in 0..3 {
        m.call("a", 0, b"hello").expect("echo");
    }
    m.call("b", 0, b"hi").expect("echo");
    let a = host.meter().usage("a");
    assert_eq!((a.calls, a.input_bytes, a.output_bytes), (3, 15, 15));
    assert!(a.fuel > 0, "the calls ran instructions");
    assert_eq!(host.meter().usage("b").calls, 1);

    // A refused input is not a call; a trapping one is, with the fuel it ran.
    let big = vec![0_u8; IO as usize + 1];
    m.call("a", 0, &big).expect_err("refused");
    assert_eq!(host.meter().usage("a").calls, 3);
    // Fuel far smaller than the deadline, so a slow runner runs out of fuel
    // before the deadline can end the call first.
    let config = HostConfig {
        fuel: 1_000_000,
        deadline_ms: 60_000,
        ..HostConfig::default()
    };
    let spinner = Host::new(config.clone());
    let spin = load(&spinner, &with_body("(loop $l (br $l)) (i32.const 0)"));
    spin.call("c", 0, b"x").expect_err("out of fuel");
    let c = spinner.meter().usage("c");
    assert_eq!((c.calls, c.input_bytes, c.output_bytes), (1, 1, 0));
    assert!(
        c.fuel >= config.fuel,
        "the whole budget was spent"
    );
}
