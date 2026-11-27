//! docs/specs/painless-compile-cache AC-4 and AC-6: the compile cache's bytes
//! come from the node's query-execution pool, and only with the switch on. A
//! process of its own, because the switch and the cache are process-wide.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use support::{request, start_with, store};

fn budget(query_pool: usize) -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::RequestBodies, 16 << 20)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, 16 << 20))
            .and_then(|c| c.with_pool(Pool::QueryExecution, query_pool))
            .expect("fits"),
    )
}

const SCRIPT: &str = r#"{"size":3,"query":{"function_score":{"query":{"match_all":{}},"boost_mode":"replace","script_score":{"script":{"source":"doc['year'].value * params.factor","params":{"factor":2}}}}}}"#;

fn search(node: &support::Node) -> u16 {
    request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(SCRIPT.as_bytes()),
    )
    .status
}

// One test: the switch is process-wide.
#[test]
fn the_cache_is_paid_for_by_the_pool_and_only_with_the_switch_on() {
    // AC-6: switch off, nothing reserved.
    oss_reader::painless_switch::set_native_compile(false);
    let off = budget(8 << 20);
    let node = start_with(store(), &off);
    assert_eq!(off.in_use(Pool::QueryExecution), 0);
    drop(node);

    // AC-4: on, the cache's bytes are held while the node lives.
    oss_reader::painless_switch::set_native_compile(true);
    let on = budget(8 << 20);
    let node = start_with(store(), &on);
    assert_eq!(
        on.in_use(Pool::QueryExecution),
        oss_reader::painless_cache::CACHE_BYTES
    );
    assert_eq!(search(&node), 200);
    assert!(oss_reader::painless_cache::bytes() > 0);

    // AC-4: a pool too small for it leaves the cache off and searches served.
    let small = budget(1 << 20);
    let node = start_with(store(), &small);
    assert_eq!(small.in_use(Pool::QueryExecution), 0);
    assert_eq!(oss_reader::painless_cache::bytes(), 0);
    assert_eq!(search(&node), 200);
    oss_reader::painless_cache::remove();
}
