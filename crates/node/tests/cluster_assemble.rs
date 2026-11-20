//! docs/specs/node-cluster-routes: the settings and cat routes an assembled node always serves, and its health routes once a source is given.

#![cfg(all(feature = "reader", feature = "writer"))]

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, ShardKey};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::assemble::{Config, Running, Writer, assemble};
use oss_node::writer::Routes;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};

struct Lease;

impl EpochLease for Lease {
    fn record(&self, _: &[NodeLogEpoch]) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Local;

fn info() -> IndexInfo {
    IndexInfo {
        name: "docs".into(),
        uuid: "docs-uuid".into(),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    }
}

impl Routes for Local {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "docs").then(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        Vec::new()
    }
}

fn budget(request_bodies: usize, open_indexes: usize) -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(512 << 20)
            .with_pool(Pool::RequestBodies, request_bodies)
            .and_then(|c| c.with_pool(Pool::OpenIndexes, open_indexes))
            .and_then(|c| c.with_pool(Pool::Transport, 8 << 20))
            .expect("fits"),
    )
}

fn config() -> Config {
    Config {
        publish_every: Duration::from_millis(10),
        publish_age_millis: 0,
        ..Config::new("", "node-1", [0; 6])
    }
}

fn writer(store: &Arc<Emulator>) -> Writer {
    let log =
        NodeLogStore::new(Arc::clone(store) as Arc<dyn BlobStore>, "", "node-1").expect("names");
    Writer {
        committer: Arc::new(NodeLogCommitter::new(
            log,
            7,
            Box::new(Arc::new(Lease)),
            Box::new(Arc::new(Admit)),
            CommitterConfig {
                window: Duration::from_millis(0),
                ..CommitterConfig::default()
            },
        )),
        routes: Arc::new(Local),
        opener: None,
        names: None,
        incarnation: None,
    }
}

fn call(r: &Running, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(r.addr(), method, target, body.map(str::as_bytes))
}

use oss_coordinator::cluster::health::{HeadRead, IndexInput};
use oss_node::health_route::HealthSource;

struct Source;

impl HealthSource for Source {
    fn nodes(&self) -> Option<usize> {
        Some(2)
    }

    fn index(&self, name: &str) -> IndexInput {
        if name == "docs" {
            IndexInput::Shards(vec![(0, HeadRead::Owned { held: true })])
        } else {
            IndexInput::Missing
        }
    }
}

#[test]
fn the_settings_and_cat_routes_are_always_served_and_health_once_a_source_is_given() {
    let store = Arc::new(Emulator::new(1 << 28, 9));
    let budget = budget(256 << 20, 16 << 20);
    let build = || {
        assemble(
            &config(),
            Arc::clone(&store) as Arc<dyn BlobStore>,
            &budget,
            writer(&store),
        )
        .expect("assembles")
    };
    let serve = |node: oss_node::assemble::Assembled| {
        node.start(
            TcpListener::bind("127.0.0.1:0").expect("binds"),
            support::server_config(),
            &budget,
        )
        .expect("starts")
    };
    let bare = serve(build());
    let put = call(
        &bare,
        "PUT",
        "/_cluster/settings",
        Some(r#"{"persistent":{"serverless.bucket":"auto"}}"#),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    let read = call(&bare, "GET", "/_cluster/settings?flat_settings=true", None);
    assert!(
        read.text().contains(r#""serverless.bucket":"auto""#),
        "{}",
        read.text()
    );
    assert_eq!(call(&bare, "GET", "/_cat/indices", None).status, 200);
    assert_eq!(call(&bare, "GET", "/_list/indices", None).status, 200);
    assert_ne!(
        call(&bare, "GET", "/_cluster/health", None).status,
        200,
        "no source, no health route"
    );

    let healthy = serve(
        build()
            .with_health(Arc::new(Source), "assembled")
            .expect("registers"),
    );
    let health = call(&healthy, "GET", "/_cluster/health/docs", None);
    assert_eq!(health.status, 200, "{}", health.text());
    assert!(
        health.text().contains(r#""number_of_nodes":2"#),
        "{}",
        health.text()
    );
    assert!(health.text().contains(r#""cluster_name":"assembled""#));
    assert_eq!(
        call(&healthy, "GET", "/_cat/health", None).text(),
        "assembled green 2 2 - -\n"
    );
    assert_eq!(
        call(&healthy, "GET", "/_cluster/health/zzz", None).status,
        404
    );
}
