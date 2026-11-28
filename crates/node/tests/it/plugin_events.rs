//! Lifecycle events delivered to a linked-in plugin (docs/specs/plugin-system
//! AC-17): in order, from a bounded queue, and a plugin that never returns
//! stalls nothing but its own queue.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_ext::CallError;
use oss_ext::lifecycle::Event;
use oss_ext::native::NativeInstance;
use oss_node::plugins::{Dispatcher, Policy, QUEUE, Registry};

const MANIFEST: &str = r#"{"plugin":{"id":"watch","version":"1.0.0","api":"1.0"},
"points":[{"name":"lifecycle","version":1}],
"events":["index.created","index.deleted"]}"#;

/// Records every index it is told of; when `gate` is set, waits on it first.
struct Watch {
    seen: Mutex<Vec<String>>,
    gate: Mutex<Option<Receiver<()>>>,
}

impl NativeInstance for Watch {
    fn manifest(&self) -> Vec<u8> {
        MANIFEST.as_bytes().to_vec()
    }

    fn call(&self, point: &str, input: &[u8], _max: usize) -> Result<Vec<u8>, CallError> {
        assert_eq!(point, "lifecycle");
        if let Some(gate) = self.gate.lock().expect("gate").as_ref() {
            let _ = gate.recv();
        }
        let e = Event::decode(input).expect("the host sends a valid event");
        self.seen.lock().expect("seen").push(e.index);
        Ok(Vec::new())
    }
}

fn dispatcher(watch: &Arc<Watch>) -> Dispatcher {
    let linked: HashMap<String, Arc<dyn NativeInstance>> =
        HashMap::from([("watch".to_owned(), watch.clone() as Arc<dyn NativeInstance>)]);
    let policy = Policy::parse(r#"{"plugins":[{"id":"watch","tier":"native"}]}"#).expect("policy");
    Dispatcher::start(&Registry::load(&policy, &linked, &[]), || 0)
}

fn event(kind: &str, index: &str) -> Event {
    Event {
        kind: kind.into(),
        tenant: "t".into(),
        index: index.into(),
        at_ms: 1,
    }
}

fn until(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn events_arrive_in_the_order_they_happened_and_only_the_subscribed_kinds() {
    let watch = Arc::new(Watch {
        seen: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let d = dispatcher(&watch);
    for i in 0..20 {
        d.publish(&event("index.created", &format!("i{i}")));
        d.publish(&event("node.started", "ignored"));
    }
    until(|| d.counts("watch").expect("lane").delivered == 20);
    let want: Vec<String> = (0..20).map(|i| format!("i{i}")).collect();
    assert_eq!(*watch.seen.lock().expect("seen"), want);
    assert_eq!(d.counts("watch").expect("lane").dropped, 0);
    assert!(d.counts("nobody").is_none());
}

#[test]
fn a_plugin_that_never_returns_delays_no_publisher_and_a_full_queue_drops_the_newest() {
    let (release, gate) = sync_channel::<()>(1024);
    let watch = Arc::new(Watch {
        seen: Mutex::new(Vec::new()),
        gate: Mutex::new(Some(gate)),
    });
    let d = dispatcher(&watch);
    let total = QUEUE + 50;
    let started = Instant::now();
    for i in 0..total {
        d.publish(&event("index.deleted", &format!("i{i}")));
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "publishing waited on a stuck plugin"
    );
    let dropped = d.counts("watch").expect("lane").dropped;
    // One event is in the worker's hands, 256 wait; the rest are dropped.
    assert!((49..=50).contains(&dropped), "dropped {dropped}");
    for _ in 0..total {
        let _ = release.send(());
    }
    until(|| d.counts("watch").expect("lane").delivered + dropped == total as u64);
    let seen = watch.seen.lock().expect("seen");
    assert_eq!(seen.first().map(String::as_str), Some("i0"));
    assert_eq!(seen.len() as u64 + dropped, total as u64);
    assert!(
        seen.windows(2).all(|w| {
            let n = |s: &str| s[1..].parse::<usize>().expect("number");
            n(&w[0]) < n(&w[1])
        }),
        "delivered in order, the dropped ones the newest"
    );
}

// docs/specs/plugin-system AC-17: the status route counts what each queue delivered.
#[test]
fn the_status_route_counts_what_a_plugins_queue_delivered() {
    use oss_node::lifecycle_bus::Bus;
    use oss_node::plugins::install;
    use oss_rest::Router;

    let watch = Arc::new(Watch {
        seen: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let linked: HashMap<String, Arc<dyn NativeInstance>> =
        HashMap::from([("watch".to_owned(), watch.clone() as Arc<dyn NativeInstance>)]);
    let backend: Arc<dyn oss_ext::host::Backend> = Arc::new(oss_ext::host::NoBackend);
    let bus = Bus::default();
    let router = install(
        Router::new(),
        r#"{"plugins":[{"id":"watch","tier":"native"}]}"#,
        &linked,
        &backend,
        || 0,
        &bus,
    )
    .expect("installs");
    let budget = oss_mem::MemoryBudget::new(&crate::support::budget_config());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        oss_rest::Server::start(listener, crate::support::server_config(), router, &budget)
            .expect("starts");
    bus.publish("index.created", "t", "a", 1);
    bus.publish("index.created", "t", "b", 2);
    bus.publish("index.purged", "t", "c", 3);
    until(|| watch.seen.lock().expect("seen").len() == 2);
    let r = crate::support::request(
        server.local_addr(),
        "GET",
        "/_plugins/_serverless/plugins",
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&r.text()).expect("json");
    assert_eq!(v["plugins"][0]["events"]["delivered"], 2, "{v}");
    assert_eq!(v["plugins"][0]["events"]["dropped"], 0);
    server.shutdown();
}
