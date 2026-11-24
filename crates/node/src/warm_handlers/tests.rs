use super::*;
use lucene_store::page_cache::PageCacheConfig;
use oss_transport::actions::warm::{HotSetResponse, PageAsk, PagesResponse};
use oss_transport::frame::VariableHeader;
use oss_transport::stream::TaskId;
use oss_transport::version::WireVersion;

struct Fleet(Arc<[NodeLease]>);

impl Leases for Fleet {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

struct Holds(Vec<(String, u32)>);

impl HeldShards for Holds {
    fn held(&self) -> Vec<HeldShard> {
        self.0
            .iter()
            .map(|(index, shard)| HeldShard {
                shard: ShardRef {
                    index_name: index.clone(),
                    index_uuid: "u".into(),
                    shard: i32::try_from(*shard).unwrap(),
                },
                prefix: format!("b/{index}/{shard}/t="),
            })
            .collect()
    }
}

fn limits() -> Limits {
    Limits {
        width: 2,
        max_shards: 1000,
        max_bytes: 1 << 30,
        max_concurrent: 2,
    }
}

fn cache(dir: &std::path::Path) -> Arc<PageCache> {
    Arc::new(
        PageCache::new(PageCacheConfig {
            disk_dir: Some(dir.to_path_buf()),
            disk_budget: 64 << 20,
            ..PageCacheConfig::default()
        })
        .expect("cache"),
    )
}

fn fleet(n: usize) -> Fleet {
    let leases: Vec<NodeLease> = (0..n)
        .map(|i| {
            let mut l = NodeLease::new(&format!("n{i}"), "e", "127.0.0.1:1", i64::MAX);
            l.set_roles([ROLE_SEARCH.to_owned()]);
            l
        })
        .collect();
    Fleet(leases.into())
}

fn ask(handler: &dyn Handler, message: &[u8]) -> Result<Vec<u8>, RemoteException> {
    let variable = VariableHeader::default();
    let request = Request {
        action: "x",
        variable: &variable,
        message,
        version: WireVersion::SHELL,
    };
    let mut sink = Vec::new();
    handler.handle(&request, &mut Output::new(&mut sink, 16 << 20))?;
    Ok(sink)
}

fn hot_ask(joiner: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    HotSetRequest {
        parent: TaskId::default(),
        joiner: joiner.into(),
        max_shards: 1000,
        max_bytes: 1 << 40,
    }
    .write(&mut Output::new(&mut buf, 4096))
    .unwrap();
    buf
}

fn put(cache: &PageCache, index: &str, shard: u32) -> String {
    let key = format!("b/{index}/{shard}/t=7\0_0.cfs\0100\0p");
    cache.insert_page(&key, 0, &[1; 50]);
    key
}

// docs/specs/reader-warm-copy AC-2
#[test]
fn a_joiner_is_listed_only_the_shards_it_is_among_the_first_readers_of() {
    let dir = oss_testkit::TempDir::new("warm-handler-place");
    let cache = cache(dir.path());
    let names: Vec<(String, u32)> = (0..40).map(|i| (format!("i{i}"), 0)).collect();
    for (index, shard) in &names {
        put(&cache, index, *shard);
    }
    let members = fleet(10);
    let handler = HotSetHandler::new(
        Arc::clone(&cache),
        Holds(names.clone()),
        fleet(10),
        limits(),
        Arc::default(),
    );

    let answer = ask(&handler, &hot_ask("n3")).expect("answers");
    let listed = HotSetResponse::read(&mut Input::new(&answer)).expect("reads");

    let expected: Vec<&str> = names
        .iter()
        .filter(|(index, shard)| {
            let key = ShardKey::new(index, i32::try_from(*shard).unwrap());
            readers_for(key, members.0.iter(), ROLE_SEARCH, 2).contains(&"n3")
        })
        .map(|(index, _)| index.as_str())
        .collect();
    let got: Vec<&str> = listed
        .shards
        .iter()
        .map(|(s, _)| s.index_name.as_str())
        .collect();
    assert!(!expected.is_empty() && expected.len() < names.len());
    assert_eq!(got, expected);
}

// docs/specs/reader-warm-copy AC-2
#[test]
fn a_joiner_the_leases_do_not_show_is_placed_as_if_it_were_a_reader() {
    let dir = oss_testkit::TempDir::new("warm-handler-new");
    let cache = cache(dir.path());
    let names: Vec<(String, u32)> = (0..60).map(|i| (format!("i{i}"), 0)).collect();
    for (index, shard) in &names {
        put(&cache, index, *shard);
    }
    let handler = HotSetHandler::new(
        Arc::clone(&cache),
        Holds(names),
        fleet(4),
        limits(),
        Arc::default(),
    );
    let answer = ask(&handler, &hot_ask("brand-new")).expect("answers");
    let listed = HotSetResponse::read(&mut Input::new(&answer)).expect("reads");
    assert!(!listed.shards.is_empty());
}

// docs/specs/reader-warm-copy AC-2
#[test]
fn the_node_limits_win_over_a_bigger_ask() {
    let dir = oss_testkit::TempDir::new("warm-handler-limits");
    let cache = cache(dir.path());
    let names: Vec<(String, u32)> = (0..30).map(|i| (format!("i{i}"), 0)).collect();
    for (index, shard) in &names {
        put(&cache, index, *shard);
    }
    let tight = Limits {
        width: 10,
        max_shards: 3,
        ..limits()
    };
    let handler = HotSetHandler::new(
        Arc::clone(&cache),
        Holds(names),
        fleet(10),
        tight,
        Arc::default(),
    );
    let answer = ask(&handler, &hot_ask("n1")).expect("answers");
    let listed = HotSetResponse::read(&mut Input::new(&answer)).expect("reads");
    assert_eq!(listed.shards.len(), 3);
}

#[test]
fn a_malformed_or_oversized_ask_is_refused() {
    let dir = oss_testkit::TempDir::new("warm-handler-bad");
    let cache = cache(dir.path());
    let hot = HotSetHandler::new(
        Arc::clone(&cache),
        Holds(Vec::new()),
        fleet(2),
        limits(),
        Arc::default(),
    );
    assert!(ask(&hot, &[1, 2, 3]).is_err());
    assert!(ask(&hot, &vec![0; MAX_HOT_SET_ASK_BYTES + 1]).is_err());
    let pages = PagesHandler::new(cache, Holds(Vec::new()), limits(), Arc::default());
    assert!(ask(&pages, &[1, 2, 3]).is_err());
    assert!(ask(&pages, &vec![0; MAX_PAGES_ASK_BYTES + 1]).is_err());
}

// docs/specs/reader-warm-copy AC-3
#[test]
fn pages_are_served_for_held_shards_and_a_key_elsewhere_is_refused() {
    let dir = oss_testkit::TempDir::new("warm-handler-pages");
    let cache = cache(dir.path());
    let mine = put(&cache, "a", 0);
    let theirs = put(&cache, "z", 0);
    let handler = PagesHandler::new(
        Arc::clone(&cache),
        Holds(vec![("a".into(), 0)]),
        limits(),
        Arc::default(),
    );
    let encode = |keys: &[&str]| {
        let mut buf = Vec::new();
        PagesRequest {
            parent: TaskId::default(),
            asks: keys
                .iter()
                .map(|k| PageAsk {
                    key: (*k).to_owned(),
                    page: 0,
                })
                .collect(),
        }
        .write(&mut Output::new(&mut buf, 1 << 16))
        .unwrap();
        buf
    };

    let answer = ask(&handler, &encode(&[&mine])).expect("answers");
    let served = PagesResponse::read(&mut Input::new(&answer)).expect("reads");
    assert_eq!(served.pages.len(), 1);
    assert_eq!(served.pages[0].bytes, vec![1; 50]);

    assert!(ask(&handler, &encode(&[&mine, &theirs])).is_err());
}

// docs/specs/reader-warm-copy AC-3
#[test]
fn a_node_serving_its_limit_refuses_the_next_request_and_takes_one_again_when_one_ends() {
    let gate = Gate::default();
    let first = gate.enter(2).expect("first");
    let second = gate.enter(2).expect("second");
    assert!(gate.enter(2).is_none());
    drop(first);
    let third = gate.enter(2).expect("a place freed");
    assert!(gate.enter(2).is_none());
    drop((second, third));
    assert!(gate.enter(0).is_none());
}

#[test]
fn a_handler_at_its_limit_answers_busy() {
    let dir = oss_testkit::TempDir::new("warm-handler-busy");
    let cache = cache(dir.path());
    let gate: Arc<Gate> = Arc::default();
    let one = Limits {
        max_concurrent: 1,
        ..limits()
    };
    let hold = gate.enter(1).expect("takes the only place");
    let handler = HotSetHandler::new(cache, Holds(Vec::new()), fleet(2), one, Arc::clone(&gate));
    assert!(ask(&handler, &hot_ask("n1")).is_err());
    drop(hold);
    assert!(ask(&handler, &hot_ask("n1")).is_ok());
}
