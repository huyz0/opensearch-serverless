//! A plugin's host operations over HTTP (docs/specs/plugin-system AC-8,
//! AC-9): what its manifest asked and the policy granted is what it gets,
//! and the node's backend answers it.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use oss_ext::CallError;
use oss_ext::host::{Backend, Host, HostError, Who};
use oss_ext::native::NativeInstance;
use oss_ext::rest::RestResponse;
use oss_mem::MemoryBudget;
use oss_node::plugins::install;
use oss_rest::{Router, Server};

use crate::support;

const MANIFEST: &str = r#"{"plugin":{"id":"asker","version":"1.0.0","api":"1.0"},
"points":[{"name":"rest","version":1}],
"routes":[{"method":"GET","path":"/_plugins/asker/go","action":"go"}],
"capabilities":["identity","search"],
"budget":{"state_ops":0,"searches":1,"egress":0}}"#;

/// Asks the host for identity, two searches (the budget is one) and a state
/// get (not granted), and answers what each came to.
struct Asker;

impl NativeInstance for Asker {
    fn manifest(&self) -> Vec<u8> {
        MANIFEST.as_bytes().to_vec()
    }

    fn call(&self, _p: &str, _i: &[u8], _m: usize) -> Result<Vec<u8>, CallError> {
        Err(CallError::malformed_because("needs a host"))
    }

    fn call_host(
        &self,
        _point: &str,
        _input: &[u8],
        _max: usize,
        host: &dyn Host,
    ) -> Result<Vec<u8>, CallError> {
        let show = |r: Result<Vec<u8>, HostError>| match r {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => format!("{}", e.status()),
        };
        let body = format!(
            "{}|{}|{}|{}",
            show(host.call("identity", b"", 64)),
            show(host.call("search", b"q1", 64)),
            show(host.call("search", b"q2", 64)),
            show(host.call("state.get", b"k", 64)),
        );
        Ok(RestResponse {
            status: 200,
            headers: vec![],
            body: body.into_bytes(),
        }
        .encode())
    }
}

/// Answers an operation with who asked and what.
struct Node;

impl Backend for Node {
    fn call(&self, who: &Who<'_>, op: &str, input: &[u8]) -> Result<Vec<u8>, HostError> {
        Ok(format!("{op}:{}:{}", who.plugin, String::from_utf8_lossy(input)).into_bytes())
    }
}

#[test]
fn a_plugin_reaches_the_backend_only_through_what_it_was_granted() {
    let linked: HashMap<String, Arc<dyn NativeInstance>> = HashMap::from([(
        "asker".to_owned(),
        Arc::new(Asker) as Arc<dyn NativeInstance>,
    )]);
    let backend: Arc<dyn Backend> = Arc::new(Node);
    let policy = r#"{"plugins":[{"id":"asker","tier":"native",
        "grants":["identity","search"]}]}"#;
    let router = install(
        Router::new(),
        policy,
        &linked,
        &backend,
        || 0,
        &oss_node::lifecycle_bus::Bus::default(),
    )
    .expect("installs");
    let budget = MemoryBudget::new(&support::budget_config());
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    let r = support::request(server.local_addr(), "GET", "/_plugins/asker/go", None);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.text(),
        "identity:asker:|search:asker:q1|429|403",
        "identity answered, the first search answered, the second over budget, state not granted"
    );
    server.shutdown();
}
