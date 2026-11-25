//! docs/specs/java-search-shard-action: the owner answers a Java shard
//! search from its own view, which holds the writes it acknowledged and has
//! not published, and makes no request to the store.

#[path = "support/mod.rs"]
mod support;

use std::io;
use std::sync::Arc;

use lucene_store::ByteBuffersDirectory;
use oss_blob::S3Op;
use oss_format::WalRecord;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::lease_age::LeaseAge;
use oss_node::search_shard_action::{Found, Limits, SearchShardHandler, Shards, run};
use oss_node::tail_overlay::install;
use oss_reader::commit_push::Stage;
use oss_reader::dsl::SearchSource;
use oss_reader::failure::SearchError;
use oss_reader::overlay::{CommitAccess, Overlay, OverlaySource, Ready, find_copies};
use oss_testkit::{TempDir, budget};
use oss_transport::frame::VariableHeader;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};
use oss_transport::version::WireVersion;
use oss_writer::mapping::Mapping;

#[path = "overlay_search_rig/mod.rs"]
mod rig;

use rig::*;

const FIXTURES: &str = include_str!("../../../fixtures/search-shard/java-3.10.0.txt");

/// The overlay of a replaced id (`a`, now `n` 7), a deleted one (`b`) and a
/// new one (`c`, `n` 9): the writes an owner acknowledged and has not
/// published.
struct Tail {
    mapping: Mapping,
}

impl OverlaySource for Tail {
    fn ready(
        &self,
        _: &str,
        _: u32,
        _: &Stage,
        commit: &Arc<dyn CommitAccess>,
        _: u64,
    ) -> Option<Arc<Ready>> {
        let records = [
            WalRecord::index("a".to_owned(), r#"{"n":7}"#.to_owned(), 1000, 1, 1001),
            WalRecord::deletion("b".to_owned(), 1001, 1, 1002),
            WalRecord::index("c".to_owned(), r#"{"n":9}"#.to_owned(), 1002, 1, 1003),
        ];
        let overlay = Overlay::build(&self.mapping, &records, 0, 4 << 20).expect("builds");
        let copies = find_copies(commit.reader(), overlay.ids()).expect("copies");
        Some(Arc::new(Ready::new(overlay, copies)))
    }

    fn closed(&self, _: &str, _: u32, _: &Stage, _: u64) {}
}

/// The shard the fixture request names (`idx`, shard 2, `uuid-1`) is the
/// fixture's shard `logs`/0.
struct Owner(Arc<oss_node::search::SearchService<oss_blob::FsStore>>);

impl Shards for Owner {
    fn uuid_of(&self, index: &str) -> Option<String> {
        (index == "idx").then(|| "uuid-1".to_owned())
    }

    fn holds(&self, _: &str, _: u32) -> bool {
        true
    }

    fn search(
        &self,
        _: &str,
        _: u32,
        source: &SearchSource,
        reserve: &dyn Fn(usize) -> io::Result<()>,
    ) -> Result<Found, Box<SearchError>> {
        run(&self.0, "logs", 0, source, reserve)
    }
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    let hex = FIXTURES
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .expect("a fixture");
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

fn ask(handler: &SearchShardHandler, name: &str) -> Vec<u8> {
    let message = fixture_bytes(name);
    let variable = VariableHeader::default();
    let request = Request {
        action: "internal:serverless/search/shard",
        variable: &variable,
        message: &message,
        version: WireVersion::SHELL,
    };
    let mut sink = Vec::new();
    handler
        .handle(&request, &mut Output::new(&mut sink, 32 << 20))
        .expect("answers");
    sink
}

fn handler(fx: &Fixture<'_>, lease: Arc<LeaseAge>) -> SearchShardHandler {
    SearchShardHandler::new(
        Arc::new(Owner(Arc::clone(&fx.service))),
        lease,
        MemoryBudget::new(
            &BudgetConfig::new(64 << 20)
                .with_pool(Pool::QueryExecution, 32 << 20)
                .expect("fits"),
        ),
        Limits {
            max_lease_age_millis: 10_000,
            max_concurrent: 4,
        },
    )
}

fn fresh() -> Arc<LeaseAge> {
    let lease = Arc::new(LeaseAge::default());
    lease.mark();
    lease
}

#[test]
fn an_owner_answers_from_its_unpublished_tail_and_asks_the_store_for_nothing() {
    let root = TempDir::new("search-shard-action");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    assert!(install(
        &fx.service,
        true,
        Arc::new(Tail {
            mapping: Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#)
                .expect("mapping"),
        })
    ));
    let handler = handler(&fx, fresh());
    // The first answer opens the shard; the next ones are the budget.
    let first = ask(&handler, "request.match_all");
    let before = counts(&fx.store);
    let answer = ask(&handler, "request.match_all");
    let after = counts(&fx.store);
    assert_eq!(first, answer);
    let requests: u64 = S3Op::ALL.iter().map(|op| gets(before, after, *op)).sum();
    budget::check("search.shard_action.requests", requests, "");
    // a (replaced, n 7) and c (new, n 9) are the shard's view; b is deleted.
    let mut input = Input::new(&answer);
    assert_eq!(input.vlong("total").expect("total"), 2);
    assert_eq!(input.vint("hits").expect("hits"), 2);
    let text = String::from_utf8_lossy(&answer);
    assert!(
        text.contains(r#"{"n":7}"#) && text.contains(r#"{"n":9}"#),
        "{text}"
    );
    assert!(!text.contains(r#"{"n":1}"#), "the replaced source is gone");
}
