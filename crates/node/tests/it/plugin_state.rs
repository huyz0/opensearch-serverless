//! What a plugin's state operations cost in object-store requests, held to
//! the rows of `docs/request-budgets.md`.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_ext::host::{Backend, NoBackend, Who};
use oss_ext::state::{Put, encode_key, encode_put};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::plugins::StateBackend;
use oss_testkit::budget;

#[test]
fn a_get_a_put_and_a_delete_cost_what_the_table_says() {
    let store = Arc::new(Emulator::new(1 << 30, 3));
    let budget_pool = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 64 << 20)
            .expect("fits"),
    );
    let state = StateBackend::new(
        Arc::clone(&store) as Arc<dyn BlobStore>,
        "",
        &budget_pool,
        Arc::new(NoBackend),
    );
    let who = Who {
        plugin: "geo",
        tenant: "t1",
        grants: &[],
    };
    let total = || -> u64 { S3Op::ALL.iter().map(|op| store.meter().count(*op)).sum() };
    let measure = |key: &str, op: &str, input: Vec<u8>| {
        let before = total();
        state.call(&who, op, &input).expect(op);
        budget::check(key, total() - before, op);
    };
    measure(
        "plugin.state_get_miss.requests",
        "state.get",
        encode_key("k"),
    );
    let put = encode_put(&Put {
        key: "k".to_owned(),
        value: b"v".to_vec(),
        if_absent: false,
    });
    measure("plugin.state_put.requests", "state.put", put);
    measure("plugin.state_get.requests", "state.get", encode_key("k"));
    measure(
        "plugin.state_delete.requests",
        "state.delete",
        encode_key("k"),
    );
}
