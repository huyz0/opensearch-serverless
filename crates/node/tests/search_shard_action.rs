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
use oss_transport::actions::search_shard::{Bounds, Request as ForwardedRequest};
use oss_transport::frame::VariableHeader;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};
use oss_transport::version::WireVersion;
use oss_writer::mapping::Mapping;

#[path = "overlay_search_rig/mod.rs"]
mod rig;

use rig::*;

const FIXTURES: &str = include_str!("../../../fixtures/search-shard/java-3.10.0.txt");
/// Answers the Java shell wrote for a shard of tied hits
/// (`fixtures/search-shard/WriteTieOrder.java`).
const TIES: &str = include_str!("../../../fixtures/search-shard/java-ties-3.10.0.txt");

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
    ask_bytes(handler, &fixture_bytes(name))
}

fn ask_bytes(handler: &SearchShardHandler, message: &[u8]) -> Vec<u8> {
    let variable = VariableHeader::default();
    let request = Request {
        action: "internal:serverless/search/shard",
        variable: &variable,
        message,
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
    // Every hit scores 1.0, so they tie: they keep the shard's document
    // order, the rewritten `a` before the new `c`, as Lucene's top docs do.
    let (seven, nine) = (text.find(r#"{"n":7}"#), text.find(r#"{"n":9}"#));
    assert!(seven < nine, "tied hits keep document order: {text}");
}

/// The `_source` documents of an answer, in the order the hits come in. Each
/// document of the tie shard carries its own `n`, so the order of the sources
/// is the order of the hits.
fn sources(answer: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(answer);
    let mut found: Vec<(usize, String)> = (1..=4)
        .filter_map(|n| {
            let source = format!(r#"{{"n":{n}}}"#);
            text.find(&source).map(|at| (at, source))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, source)| source).collect()
}

fn java_answer(name: &str) -> Vec<u8> {
    java_line(name)
}

/// A request or an answer the Java shell wrote, by its line's name.
fn java_line(name: &str) -> Vec<u8> {
    let hex = TIES
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .expect("a Java answer");
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

/// The shard the Java shell answered for: `z m q` published, then `m`
/// rewritten, `q` deleted, `a` and `k` added. Document order is `z m a k`;
/// `_id` order would be `a k m z`.
struct TieTail {
    mapping: Mapping,
}

impl OverlaySource for TieTail {
    fn ready(
        &self,
        _: &str,
        _: u32,
        _: &Stage,
        commit: &Arc<dyn CommitAccess>,
        _: u64,
    ) -> Option<Arc<Ready>> {
        let records = [
            WalRecord::index("m".to_owned(), r#"{"n":2}"#.to_owned(), 1000, 1, 1001),
            WalRecord::deletion("q".to_owned(), 1001, 1, 1002),
            WalRecord::index("a".to_owned(), r#"{"n":3}"#.to_owned(), 1002, 1, 1003),
            WalRecord::index("k".to_owned(), r#"{"n":4}"#.to_owned(), 1003, 1, 1004),
        ];
        let overlay = Overlay::build(&self.mapping, &records, 0, 4 << 20).expect("builds");
        let copies = find_copies(commit.reader(), overlay.ids()).expect("copies");
        Some(Arc::new(Ready::new(overlay, copies)))
    }

    fn closed(&self, _: &str, _: u32, _: &Stage, _: u64) {}
}

#[test]
fn tied_hits_come_in_the_order_the_java_shell_answers_them() {
    let root = TempDir::new("search-shard-ties");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["z", "m", "q"]);
    assert!(install(
        &fx.service,
        true,
        Arc::new(TieTail {
            mapping: Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#)
                .expect("mapping"),
        })
    ));
    let handler = handler(&fx, fresh());
    let mut sized = ForwardedRequest::read(&fixture_bytes("request.match_all"), &Bounds::DEFAULT)
        .expect("reads");
    sized.source.size = 2;
    let mut two = Vec::new();
    sized
        .write(&mut Output::new(&mut two, 1 << 20))
        .expect("writes");
    for (name, request) in [
        ("response.match_all", fixture_bytes("request.match_all")),
        ("response.match_all_size_2", two),
    ] {
        let java = java_answer(name);
        let rust = ask_bytes(&handler, &request);
        assert_eq!(sources(&rust), sources(&java), "{name}: hit order");
        let (mut j, mut r) = (Input::new(&java), Input::new(&rust));
        assert_eq!(
            r.vlong("total").expect("total"),
            j.vlong("total").expect("total"),
            "{name}: total"
        );
        assert_eq!(
            r.vint("hits").expect("hits"),
            j.vint("hits").expect("hits"),
            "{name}: hits"
        );
    }
    // Document order, not `_id` order, and the Java shard's own answer says so.
    assert_eq!(
        sources(&java_answer("response.match_all")),
        [r#"{"n":1}"#, r#"{"n":2}"#, r#"{"n":3}"#, r#"{"n":4}"#]
    );
}

/// One hit of an answer: its id and the bits of its score.
type Hit = (String, u32);

/// The hits of an answer in the order they come in, then the shard's total
/// and the bits of its best score. The tie tests compare these with the
/// Java shell's own answer, so the order and every score are held, not only
/// the order.
fn decode(answer: &[u8]) -> (i64, Vec<Hit>, u32) {
    let mut input = Input::new(answer);
    let total = input.vlong("total").expect("total");
    let count = input.vint("hits").expect("hits");
    let mut hits = Vec::new();
    for _ in 0..count {
        let score = input.take(4, "score").expect("score");
        let score = u32::from_be_bytes(score.try_into().expect("four bytes"));
        let len = usize::try_from(input.int("id length").expect("id length")).expect("positive");
        let id = String::from_utf8(input.take(len, "id").expect("id").to_vec()).expect("utf8");
        input.bool("identity").expect("identity");
        input.long("version").expect("version");
        input.zlong("seq no").expect("seq no");
        input.vlong("primary term").expect("primary term");
        input.bytes_reference("source").expect("source");
        input.bool("explanation").expect("explanation");
        for _ in 0..6 {
            input.vint("empty map").expect("empty map");
        }
        // Java writes the shard it ran on (a node id flag, index, uuid,
        // shard, cluster alias flag); Rust writes none.
        if input.bool("shard target").expect("shard target") {
            input.bool("node id").expect("node id");
            input.string("index").expect("index");
            input.string("uuid").expect("uuid");
            input.vint("shard").expect("shard");
            input.bool("cluster alias").expect("cluster alias");
        }
        input.vint("inner hits").expect("inner hits");
        hits.push((id, score));
    }
    input.bool("aggregations").expect("aggregations");
    input.bool("lower bound").expect("lower bound");
    let max = input.take(4, "max score").expect("max score");
    (
        total,
        hits,
        u32::from_be_bytes(max.try_into().expect("four bytes")),
    )
}

const SEGMENT_ONE: [(&str, &str); 3] = [
    ("z", r#"{"n":1,"msg":"alpha"}"#),
    ("m", r#"{"n":2,"msg":"alpha beta"}"#),
    ("q", r#"{"n":3,"msg":"alpha"}"#),
];
const SEGMENT_TWO: [(&str, &str); 3] = [
    ("a", r#"{"n":4,"msg":"alpha beta"}"#),
    ("k", r#"{"n":5,"msg":"alpha gamma delta"}"#),
    ("y", r#"{"n":6,"msg":"alpha"}"#),
];

/// The second segment as writes the owner has not published.
struct SecondSegmentTail {
    mapping: Mapping,
}

impl OverlaySource for SecondSegmentTail {
    fn ready(
        &self,
        _: &str,
        _: u32,
        _: &Stage,
        commit: &Arc<dyn CommitAccess>,
        _: u64,
    ) -> Option<Arc<Ready>> {
        let records: Vec<WalRecord> = SEGMENT_TWO
            .iter()
            .enumerate()
            .map(|(at, (id, source))| {
                let at = i64::try_from(at).expect("small");
                WalRecord::index(
                    (*id).to_owned(),
                    (*source).to_owned(),
                    at.saturating_add(1000),
                    1,
                    at.saturating_add(1001),
                )
            })
            .collect();
        let overlay = Overlay::build(&self.mapping, &records, 0, 4 << 20).expect("builds");
        let copies = find_copies(commit.reader(), overlay.ids()).expect("copies");
        Some(Arc::new(Ready::new(overlay, copies)))
    }

    fn closed(&self, _: &str, _: u32, _: &Stage, _: u64) {}
}

/// Asks `handler` what the Java shell was asked and holds every answer to
/// the Java one: total, hit order, every score and the best score.
fn assert_java_answers(handler: &SearchShardHandler, asked: &[(&str, Vec<u8>)]) {
    for (name, request) in asked {
        let java = decode(&java_answer(&format!("response.{name}")));
        let rust = decode(&ask_bytes(handler, request));
        assert_eq!(rust, java, "{name}");
    }
}

/// The requests the Java shell wrote and the answers it gave to them.
fn tie_requests() -> Vec<(&'static str, Vec<u8>)> {
    [
        "segments_match_all",
        "segments_match_all_size_4",
        "scored_term",
        "scored_term_size_2",
        "scored_term_size_4",
    ]
    .map(|name| (name, java_line(&format!("request.{name}"))))
    .to_vec()
}

fn text_fixture<'d>(
    root: &std::path::Path,
    dir: &'d ByteBuffersDirectory,
    second_published: bool,
) -> Fixture<'d> {
    fixture_with(
        root,
        dir,
        (TEXT_DESCRIPTOR, TEXT_MAPPING),
        "reader-1",
        |w| {
            w.publish_docs(&SEGMENT_ONE);
            if second_published {
                w.publish_docs(&SEGMENT_TWO);
            }
        },
    )
}

#[test]
fn ties_across_two_published_segments_and_below_one_come_as_the_java_shell_answers() {
    let root = TempDir::new("search-shard-segment-ties");
    let dir = ByteBuffersDirectory::new();
    let fx = text_fixture(root.path(), &dir, true);
    assert_java_answers(&handler(&fx, fresh()), &tie_requests());
    // Equal scores below 1.0 come in document order, and the Java answer
    // says so: the three one-word documents, then the two of two words.
    let (_, hits, _) = decode(&java_answer("response.scored_term"));
    let ids: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["z", "q", "y", "m", "a", "k"]);
    assert_eq!(hits[0].1, hits[2].1);
    assert_ne!(hits[0].1, hits[3].1);
}

#[test]
fn the_same_ties_with_the_second_segment_in_the_tail_answer_as_the_java_shell_does() {
    let root = TempDir::new("search-shard-tail-segment-ties");
    let dir = ByteBuffersDirectory::new();
    let fx = text_fixture(root.path(), &dir, false);
    assert!(install(
        &fx.service,
        true,
        Arc::new(SecondSegmentTail {
            mapping: Mapping::parse(TEXT_MAPPING.as_bytes()).expect("mapping"),
        })
    ));
    assert_java_answers(&handler(&fx, fresh()), &tie_requests());
}
