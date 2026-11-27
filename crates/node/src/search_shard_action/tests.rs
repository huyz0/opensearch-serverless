use std::time::Duration;

use oss_mem::BudgetConfig;
use oss_transport::frame::VariableHeader;
use oss_transport::stream::Input;

use super::*;

const FIXTURES: &str = include_str!("../../../../fixtures/search-shard/java-3.10.0.txt");

fn fixture(name: &str) -> Vec<u8> {
    let hex = FIXTURES
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .expect("a fixture");
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

struct Fake {
    held: bool,
    uuid: &'static str,
    ran: std::sync::Mutex<Vec<(String, u32)>>,
    wait: Duration,
    fail: Option<u16>,
    now: std::sync::Mutex<Vec<Option<i64>>>,
    aggs: Vec<oss_reader::aggs::partial::Partial>,
}

impl Fake {
    fn held() -> Self {
        Self {
            held: true,
            uuid: "uuid-1",
            ran: std::sync::Mutex::default(),
            wait: Duration::ZERO,
            fail: None,
            now: std::sync::Mutex::default(),
            aggs: Vec::new(),
        }
    }
}

impl Shards for Fake {
    fn uuid_of(&self, index: &str) -> Option<String> {
        (index == "idx").then(|| self.uuid.to_owned())
    }

    fn holds(&self, uuid: &str, _: u32) -> bool {
        self.held && uuid == self.uuid
    }

    fn search(
        &self,
        index: &str,
        shard: u32,
        source: &SearchSource,
        reserve: &dyn Fn(usize) -> io::Result<()>,
    ) -> Result<Found, Box<SearchError>> {
        self.ran.lock().unwrap().push((index.to_owned(), shard));
        self.now.lock().unwrap().push(source.now_millis);
        std::thread::sleep(self.wait);
        if let Some(status) = self.fail {
            return Err(Box::new(SearchError::request(
                status,
                oss_reader::failure::Failure::plain("x", "boom".to_owned()),
            )));
        }
        reserve(8).map_err(|e| {
            Box::new(SearchError::request(
                429,
                oss_reader::failure::Failure::plain("x", e.to_string()),
            ))
        })?;
        Ok(Found {
            total: 42,
            exact: true,
            max_score: None,
            terminated_early: None,
            aggregations: self.aggs.clone(),
            rows: vec![
                Row {
                    id_json: b"\"a\"".to_vec(),
                    score: Some(1.5),
                    version: Some(3),
                    seq_no: Some(7),
                    primary_term: Some(2),
                    source: b"{\"n\":1}".to_vec(),
                },
                Row {
                    id_json: b"\"b\"".to_vec(),
                    score: Some(0.25),
                    version: Some(3),
                    seq_no: Some(8),
                    primary_term: Some(2),
                    source: b"{\"n\":2}".to_vec(),
                },
            ],
        })
    }
}

fn handler(fake: Fake, lease: &Arc<LeaseAge>, max_age: u64) -> SearchShardHandler {
    shared(Arc::new(fake), lease, max_age)
}

fn shared(fake: Arc<Fake>, lease: &Arc<LeaseAge>, max_age: u64) -> SearchShardHandler {
    SearchShardHandler::new(
        fake,
        Arc::clone(lease),
        MemoryBudget::new(
            &BudgetConfig::new(64 << 20)
                .with_pool(Pool::QueryExecution, 32 << 20)
                .unwrap(),
        ),
        Limits {
            max_lease_age_millis: max_age,
            max_concurrent: 2,
        },
    )
}

fn ask(
    handler: &SearchShardHandler,
    message: &[u8],
    version: WireVersion,
) -> Result<Vec<u8>, RemoteException> {
    let variable = VariableHeader::default();
    let request = Request {
        action: "x",
        variable: &variable,
        message,
        version,
    };
    let mut sink = Vec::new();
    handler.handle(&request, &mut Output::new(&mut sink, 32 << 20))?;
    Ok(sink)
}

fn fresh() -> Arc<LeaseAge> {
    let lease = Arc::new(LeaseAge::default());
    lease.mark();
    lease
}

#[test]
fn a_held_shard_with_a_fresh_lease_answers_in_the_java_layout() {
    let h = handler(Fake::held(), &fresh(), 10_000);
    let answer = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap();
    let mut input = Input::new(&answer);
    assert_eq!(input.vlong("total").unwrap(), 42);
    assert_eq!(input.vint("hits").unwrap(), 2);
    // The bytes the Java shell writes for the same answer.
    let expected = fixture("response.two_hits");
    assert_eq!(&answer[..], &expected[..]);
}

#[test]
fn a_shard_not_held_is_refused_before_anything_is_searched() {
    let mut fake = Fake::held();
    fake.held = false;
    let fake = Arc::new(fake);
    let h = shared(Arc::clone(&fake), &fresh(), 10_000);
    let e = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap_err();
    assert!(format!("{e:?}").contains("does not hold"));
    assert!(fake.ran.lock().unwrap().is_empty());
}

#[test]
fn another_uuid_is_refused_before_anything_is_searched() {
    let mut fake = Fake::held();
    fake.uuid = "uuid-2";
    let fake = Arc::new(fake);
    let h = shared(Arc::clone(&fake), &fresh(), 10_000);
    let e = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap_err();
    assert!(format!("{e:?}").contains("recreated"));
    assert!(fake.ran.lock().unwrap().is_empty());
}

#[test]
fn a_node_that_never_renewed_or_renewed_long_ago_refuses() {
    let never = Arc::new(LeaseAge::default());
    let h = handler(Fake::held(), &never, 10_000);
    assert!(ask(&h, &fixture("request.match_all"), WireVersion::SHELL).is_err());
    let old = fresh();
    std::thread::sleep(Duration::from_millis(30));
    let h = handler(Fake::held(), &old, 5);
    let e = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap_err();
    assert!(format!("{e:?}").contains("renewed"));
}

#[test]
fn a_search_that_outlasts_the_lease_answers_nothing() {
    let mut fake = Fake::held();
    fake.wait = Duration::from_millis(60);
    let h = handler(fake, &fresh(), 30);
    assert!(ask(&h, &fixture("request.match_all"), WireVersion::SHELL).is_err());
}

#[test]
fn an_older_peer_an_unknown_index_and_an_unsupported_query_are_refused_by_name() {
    let h = handler(Fake::held(), &fresh(), 10_000);
    assert!(ask(&h, &fixture("request.match_all"), WireVersion::MIN_COMPAT).is_err());
    let e = ask(&h, &fixture("request.match"), WireVersion::SHELL).unwrap_err();
    assert!(format!("{e:?}").contains("not supported"));
    assert!(ask(&h, &[1, 2, 3], WireVersion::SHELL).is_err());
}

#[test]
fn a_full_pool_refuses_the_search_and_a_finished_one_frees_its_slot() {
    let h = handler(Fake::held(), &fresh(), 10_000);
    let one = h.slot().unwrap();
    let two = h.slot().unwrap();
    assert!(h.slot().is_none());
    let e = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap_err();
    assert!(format!("{e:?}").contains("as many"));
    drop(one);
    assert!(ask(&h, &fixture("request.match_all"), WireVersion::SHELL).is_ok());
    drop(two);
    assert_eq!(h.running.load(Ordering::Acquire), 0);
}

#[test]
fn a_search_error_is_the_status_its_failure_names() {
    for (code, name) in [
        (400, "BAD_REQUEST"),
        (404, "NOT_FOUND"),
        (429, "TOO_MANY"),
        (503, "SERVICE_UNAVAILABLE"),
    ] {
        let mut fake = Fake::held();
        fake.fail = Some(code);
        let h = handler(fake, &fresh(), 10_000);
        let e = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap_err();
        assert!(format!("{e:?}").contains(name), "{code}: {e:?}");
    }
}

#[test]
fn the_pool_gives_back_what_a_search_reserved() {
    let h = handler(Fake::held(), &fresh(), 10_000);
    assert!(ask(&h, &fixture("request.match_all"), WireVersion::SHELL).is_ok());
    assert_eq!(h.budget.in_use(Pool::QueryExecution), 0);
}

#[test]
fn the_search_runs_at_the_instant_the_request_carries() {
    let fake = Arc::new(Fake::held());
    let h = shared(Arc::clone(&fake), &fresh(), 10_000);
    let message = fixture("request.match_all");
    let asked = oss_transport::actions::search_shard::Request::read(
        &message,
        &oss_transport::actions::search_shard::Bounds::DEFAULT,
    )
    .unwrap()
    .now_millis;
    ask(&h, &message, WireVersion::SHELL).unwrap();
    assert_eq!(*fake.now.lock().unwrap(), vec![Some(asked)]);
}

#[test]
fn aggregations_asked_for_come_back_as_internal_aggregations() {
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    use oss_transport::actions::search_shard::{AggKind, AggRequest, Request as Wire, Source};
    let mut fake = Fake::held();
    fake.aggs = vec![
        Partial {
            name: "m".to_owned(),
            format: Format::Raw,
            value: Part::Max(2.0),
        },
        Partial {
            name: "c".to_owned(),
            format: Format::Raw,
            value: Part::ValueCount(7),
        },
    ];
    let h = handler(fake, &fresh(), 10_000);
    let mut ask_bytes = Wire::read(&fixture("request.match_all"), &Bounds::DEFAULT).unwrap();
    let source: &mut Source = &mut ask_bytes.source;
    source.aggregations = vec![
        AggRequest {
            name: "m".to_owned(),
            kind: AggKind::Max,
            field: "n".to_owned(),
        },
        AggRequest {
            name: "c".to_owned(),
            kind: AggKind::ValueCount,
            field: "tag".to_owned(),
        },
    ];
    let mut message = Vec::new();
    ask_bytes
        .write(&mut Output::new(&mut message, 1 << 20))
        .unwrap();
    let answer = ask(&h, &message, WireVersion::SHELL).unwrap();
    let mut expected = vec![
        1, 2, 3, b'm', b'a', b'x', 1, b'm', 0xFF, 3, b'r', b'a', b'w',
    ];
    expected.extend_from_slice(&2.0f64.to_bits().to_be_bytes());
    expected.push(11);
    expected.extend_from_slice(b"value_count");
    expected.extend_from_slice(&[1, b'c', 0xFF, 7]);
    assert!(
        answer.windows(expected.len()).any(|w| w == &expected[..]),
        "{answer:?}"
    );
}

#[test]
fn a_date_aggregation_carries_the_date_format_and_another_kind_is_refused() {
    use oss_reader::aggs::partial::{Format, Partial, Value as Part};
    let mut fake = Fake::held();
    fake.aggs = vec![Partial {
        name: "d".to_owned(),
        format: Format::Date,
        value: Part::Max(1.0),
    }];
    let h = handler(fake, &fresh(), 10_000);
    let answer = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap();
    assert!(answer.windows(9).any(|w| w == b"date_time"), "{answer:?}");
    let mut fake = Fake::held();
    fake.aggs = vec![Partial {
        name: "t".to_owned(),
        format: Format::Raw,
        value: Part::Stats {
            count: 1,
            sum: 1.0,
            min: 1.0,
            max: 1.0,
        },
    }];
    let h = handler(fake, &fresh(), 10_000);
    let e = ask(&h, &fixture("request.match_all"), WireVersion::SHELL).unwrap_err();
    assert!(
        format!("{e:?}").contains("not supported by this node"),
        "{e:?}"
    );
}
