//! docs/specs/extension-analyzer AC-4, AC-5, AC-7, AC-8: a create may pin
//! analyzer modules, through a policy the node supplies.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_ext::pin::{Pin, ResolvedLimits};
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore, PitClock};
use oss_node::index_route::{IndexService, routes};
use oss_node::pin_policy::PinPolicy;
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Policy {
    allow: bool,
    refuse: Option<&'static str>,
    calls: AtomicUsize,
}

impl PinPolicy for Policy {
    fn authorised(&self, _principal: Option<&str>) -> bool {
        self.allow
    }

    fn check(&self, _pin: &Pin) -> Result<ResolvedLimits, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.refuse {
            Some(why) => Err(why.to_owned()),
            None => Ok(ResolvedLimits {
                fuel: 7,
                memory_bytes: 8,
                deadline_ms: 9,
            }),
        }
    }
}

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    descriptors: DescriptorStore,
    policy: Option<Arc<Policy>>,
}

fn rig(policy: Option<(bool, Option<&'static str>)>) -> Rig {
    let budget = MemoryBudget::new(&support::budget_config());
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let clock: PitClock = Arc::new(|| 1);
    let descriptors = || {
        DescriptorStore::new(
            Arc::clone(&store) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    };
    let policy = policy.map(|(allow, refuse)| {
        Arc::new(Policy {
            allow,
            refuse,
            calls: AtomicUsize::new(0),
        })
    });
    let mut service = IndexService::new(descriptors(), clock, BUF);
    if let Some(p) = &policy {
        service = service.with_pins(Arc::clone(p) as Arc<dyn PinPolicy>);
    }
    let router = routes(Router::new(), Arc::new(service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        descriptors: descriptors(),
        store,
        policy,
    }
}

fn put(r: &Rig, name: &str, body: &str) -> support::Reply {
    support::request(
        r.server.local_addr(),
        "PUT",
        &format!("/{name}"),
        Some(body.as_bytes()),
    )
}

fn calls(r: &Rig) -> usize {
    r.policy
        .as_ref()
        .map_or(0, |p| p.calls.load(Ordering::SeqCst))
}

fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

fn settings_of(r: &Rig, name: &str) -> Option<String> {
    let mut buf = vec![0u8; BUF];
    r.descriptors
        .get(name, &mut buf)
        .expect("read")
        .map(|rec| String::from_utf8_lossy(rec.settings).into_owned())
}

fn body(extra: &str, mapping: &str) -> String {
    format!(
        r#"{{"settings":{{"index.analysis.analyzer.ext_stem.type":"serverless_module","index.analysis.analyzer.ext_stem.module":"{HASH}","index.analysis.analyzer.ext_stem.version":"1"{extra}}},"mappings":{mapping}}}"#
    )
}

const USES: &str = r#"{"properties":{"t":{"type":"text","analyzer":"ext_stem"}}}"#;

// AC-4, AC-5
#[test]
fn a_create_with_a_pin_writes_the_pin_and_its_resolved_limits() {
    let r = rig(Some((true, None)));
    let reply = put(&r, "logs", &body("", USES));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(calls(&r), 1);
    let s = settings_of(&r, "logs").expect("created");
    for want in [
        format!(r#""index.analysis.analyzer.ext_stem.module":"{HASH}""#),
        r#""index.analysis.analyzer.ext_stem.type":"serverless_module""#.to_owned(),
        r#""index.analysis.analyzer.ext_stem.version":"1""#.to_owned(),
        r#""index.analysis.analyzer.ext_stem.limits.fuel":"7""#.to_owned(),
        r#""index.analysis.analyzer.ext_stem.limits.memory_bytes":"8""#.to_owned(),
        r#""index.analysis.analyzer.ext_stem.limits.deadline_ms":"9""#.to_owned(),
    ] {
        assert!(s.contains(&want), "{want} in {s}");
    }
}

#[test]
fn a_pin_beside_the_bucket_settings_keeps_both() {
    let r = rig(Some((true, None)));
    let extra = r#","index.serverless.bucket_group":"g1""#;
    assert_eq!(put(&r, "logs", &body(extra, USES)).status, 200);
    let s = settings_of(&r, "logs").expect("created");
    assert!(s.contains(r#""index.serverless.bucket_group":"g1""#), "{s}");
    assert!(s.contains("ext_stem.module"), "{s}");
}

// AC-7: no policy, no authority, a refusing module: nothing is written
#[test]
fn a_pin_is_refused_without_a_policy_without_authority_or_when_the_check_refuses() {
    let none = rig(None);
    let e = put(&none, "a", &body("", USES));
    assert_eq!(e.status, 400, "{}", e.text());
    assert!(e.text().contains("not enabled"), "{}", e.text());
    assert_eq!(puts(&none), 0);

    let denied = rig(Some((false, None)));
    let e = put(&denied, "a", &body("", USES));
    assert_eq!(e.status, 403, "{}", e.text());
    assert_eq!((calls(&denied), puts(&denied)), (0, 0));

    let refused = rig(Some((true, Some("the module is not in the store"))));
    let e = put(&refused, "a", &body("", USES));
    assert_eq!(e.status, 400, "{}", e.text());
    assert!(e.text().contains("not in the store"), "{}", e.text());
    assert_eq!(puts(&refused), 0);
}

#[test]
fn a_client_cannot_set_the_limits_or_another_analysis_key() {
    let r = rig(Some((true, None)));
    for extra in [
        r#","index.analysis.analyzer.ext_stem.limits.fuel":"1""#,
        r#","index.analysis.analyzer.ext_stem.tokenizer":"x""#,
        r#","index.analysis.filter.f.type":"x""#,
    ] {
        let e = put(&r, "a", &body(extra, USES));
        assert_eq!(e.status, 400, "{extra}: {}", e.text());
    }
    assert_eq!((calls(&r), puts(&r)), (0, 0));
}

#[test]
fn a_mapping_may_use_only_the_names_the_create_pins() {
    let r = rig(Some((true, None)));
    let other = r#"{"properties":{"t":{"type":"text","analyzer":"ext_other"}}}"#;
    let e = put(&r, "a", &body("", other));
    assert_eq!(e.status, 400, "{}", e.text());
    assert!(e.text().contains("ext_other"), "{}", e.text());
    assert_eq!(puts(&r), 0);
    // Without any pin the name is as unsupported as it was.
    let e = put(&r, "b", &format!(r#"{{"mappings":{USES}}}"#));
    assert_eq!(e.status, 400, "{}", e.text());
    assert!(e.text().contains("ext_stem"), "{}", e.text());
    // A pin that no field uses is allowed.
    let plain = r#"{"properties":{"t":{"type":"text"}}}"#;
    assert_eq!(put(&r, "c", &body("", plain)).status, 200);
}

// AC-8: a create with no pin is as it was, with or without a policy
#[test]
fn a_create_with_no_pin_never_asks_the_policy() {
    let r = rig(Some((true, None)));
    assert_eq!(
        put(&r, "plain", r#"{"properties":{"t":{"type":"text"}}}"#).status,
        200
    );
    assert_eq!(calls(&r), 0);
    assert_eq!(settings_of(&r, "plain").as_deref(), Some("{}"));
}

/// What a Rust create with a pin wrote for `logs`, for a Java node to read
/// (`fixtures/extensions/`, `PinnedDescriptorRefusedTests.java`). With
/// `OSS_WRITE_FIXTURES` set the file is rewritten; otherwise the test checks
/// it is what a create writes now.
#[test]
fn a_pinned_descriptor_is_what_the_java_fixture_holds() {
    let r = rig(Some((true, None)));
    assert_eq!(put(&r, "logs", &body("", USES)).status, 200);
    let key = oss_format::descriptor_key("", "logs").expect("key");
    let mut buf = vec![0u8; BUF];
    let found = r
        .store
        .read_register(&key, &mut buf)
        .expect("reads")
        .expect("register");
    let bytes = &buf[..found.len];
    let dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/extensions");
    if std::env::var_os("OSS_WRITE_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("pinned-descriptor.bin"), bytes).expect("writes");
    }
    let held =
        std::fs::read(dir.join("pinned-descriptor.bin")).expect("run with OSS_WRITE_FIXTURES=1");
    // The uuid and the creation time differ run to run; the settings and the mapping must not.
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let (now, then) = (text(bytes), text(&held));
    for part in [
        "\"settings\"",
        "serverless_module",
        HASH,
        "\"limits.fuel",
        "ext_stem",
    ] {
        assert_eq!(now.contains(part), then.contains(part), "{part}");
    }
    assert!(then.contains("serverless_module"), "{then}");
}
