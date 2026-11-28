//! docs/specs/ml-commons slice 3: a local model's file arrives by `upload_chunk`.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::ml_route::{Local, MlService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const BUF: usize = 1 << 16;
const ML: &str = "/_plugins/_ml";

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    models: MemoryBudget,
}

fn rig() -> Rig {
    with(4 << 20, 8, true)
}

/// A node whose models pool holds `pool` bytes and `max` models; `runtime` says whether
/// it has a model runtime configured.
fn with(pool: usize, max: usize, runtime: bool) -> Rig {
    let models = MemoryBudget::new(
        &BudgetConfig::new(1 << 30)
            .with_pool(Pool::Models, pool)
            .expect("fits"),
    );
    let path = runtime.then(|| "/opt/onnx/libonnxruntime.so".into());
    let local = Local::new(&models, path).with_max_models(max);
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = MlService::new(&blobs, "", BUF)
        .with_node_id("node-a")
        .with_local(local);
    let router = routes(Router::new(), &Arc::new(service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        models,
    }
}

fn send(r: &Rig, path: &str, body: &[u8]) -> support::Reply {
    support::request(
        r.server.local_addr(),
        "POST",
        &format!("{ML}/{path}"),
        Some(body),
    )
}

fn json_of(reply: &support::Reply) -> Value {
    assert_eq!(reply.status, 200, "{}", reply.text());
    serde_json::from_str(&reply.text()).unwrap()
}

fn meta(r: &Rig, chunks: u64) -> String {
    let body = json!({"name": "m", "function_name": "TEXT_EMBEDDING", "total_chunks": chunks,
                      "model_format": "ONNX", "version": "1"});
    let out = json_of(&send(r, "models/meta", body.to_string().as_bytes()));
    assert_eq!(out["status"], "CREATED");
    out["model_id"].as_str().unwrap().to_owned()
}

fn state(r: &Rig, id: &str) -> String {
    let reply = support::request(
        r.server.local_addr(),
        "GET",
        &format!("{ML}/models/{id}"),
        None,
    );
    json_of(&reply)["model_state"].as_str().unwrap().to_owned()
}

#[test]
fn chunks_land_as_blobs_and_the_last_one_registers_the_model() {
    let r = rig();
    let id = meta(&r, 2);
    assert_eq!(state(&r, &id), "REGISTERING");
    // Bigger than one body buffer, so the copy into the blob spans several fills.
    let first: Vec<u8> = (0..200_000_u32).map(|i| (i % 251) as u8).collect();
    let out = json_of(&send(&r, &format!("models/{id}/upload_chunk/0"), &first));
    assert_eq!(out["status"], "Uploaded");
    assert_eq!(state(&r, &id), "REGISTERING");
    json_of(&send(&r, &format!("models/{id}/upload_chunk/1"), b"tail"));
    assert_eq!(state(&r, &id), "REGISTERED");
    let mut got = Vec::new();
    let mut buf = vec![0u8; BUF];
    let mut sink = |c: &[u8]| {
        got.extend_from_slice(c);
        Ok(())
    };
    r.store
        .get(
            &format!("/ml/artifacts/{id}/00000000"),
            oss_blob::ByteRange::Full,
            &mut buf,
            &mut sink,
        )
        .expect("the chunk blob");
    assert_eq!(got, first);
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

#[test]
fn a_chunk_out_of_range_or_for_a_missing_model_or_empty_is_refused() {
    let r = rig();
    let id = meta(&r, 1);
    assert_eq!(
        send(&r, &format!("models/{id}/upload_chunk/1"), b"x").status,
        400
    );
    assert_eq!(
        send(&r, &format!("models/{id}/upload_chunk/0"), b"").status,
        400
    );
    assert_eq!(send(&r, "models/nope/upload_chunk/0", b"x").status, 404);
    assert_eq!(
        send(&r, "models/meta", br#"{"name":"m","total_chunks":0}"#).status,
        400
    );
    assert_eq!(state(&r, &id), "REGISTERING");
}

fn upload(r: &Rig, id: &str, n: u64, bytes: usize) {
    json_of(&send(
        r,
        &format!("models/{id}/upload_chunk/{n}"),
        &vec![7u8; bytes],
    ));
}

fn deploy(r: &Rig, id: &str) -> support::Reply {
    send(r, &format!("models/{id}/_deploy"), b"")
}

#[test]
fn a_node_with_no_runtime_refuses_local_models() {
    let r = with(4 << 20, 8, false);
    let body = json!({"name": "m", "total_chunks": 1});
    assert_eq!(
        send(&r, "models/meta", body.to_string().as_bytes()).status,
        400
    );
}

#[test]
fn deploy_charges_twice_the_file_before_it_loads_and_undeploy_returns_it() {
    let r = with(2000, 8, true);
    let id = meta(&r, 1);
    assert_eq!(deploy(&r, &id).status, 400, "a file not fully uploaded");
    upload(&r, &id, 0, 1000);
    assert_eq!(r.models.in_use(Pool::Models), 0);
    json_of(&deploy(&r, &id));
    assert_eq!(r.models.in_use(Pool::Models), 2000);
    assert_eq!(state(&r, &id), "DEPLOYED");
    json_of(&send(&r, &format!("models/{id}/_undeploy"), b""));
    assert_eq!(r.models.in_use(Pool::Models), 0);
}

#[test]
fn a_model_past_the_pool_or_the_count_is_refused_429_and_charges_nothing() {
    let r = with(1999, 8, true);
    let id = meta(&r, 1);
    upload(&r, &id, 0, 1000);
    assert_eq!(deploy(&r, &id).status, 429);
    assert_eq!(r.models.in_use(Pool::Models), 0);
    assert_eq!(state(&r, &id), "REGISTERED");

    let r = with(1 << 20, 1, true);
    let (a, b) = (meta(&r, 1), meta(&r, 1));
    upload(&r, &a, 0, 10);
    upload(&r, &b, 0, 10);
    json_of(&deploy(&r, &a));
    assert_eq!(deploy(&r, &b).status, 429);
    assert_eq!(r.models.in_use(Pool::Models), 20);
}

#[test]
fn deleting_a_model_deletes_its_chunks_without_a_list() {
    let r = rig();
    let id = meta(&r, 2);
    upload(&r, &id, 0, 10);
    upload(&r, &id, 1, 10);
    let key = format!("/ml/artifacts/{id}/00000001");
    let head = |r: &Rig| r.store.head(&key).expect("head").is_some();
    assert!(head(&r));
    let del = support::request(
        r.server.local_addr(),
        "DELETE",
        &format!("{ML}/models/{id}"),
        None,
    );
    assert_eq!(del.status, 200, "{}", del.text());
    assert!(!head(&r));
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}
