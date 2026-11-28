//! docs/specs/replication/SPEC.md AC-1, AC-5 and AC-6: `cluster.remote.<alias>.store`
//! through the real settings route.

#![cfg(all(feature = "reader", feature = "writer"))]

use crate::cluster_assemble::{budget, call, config, writer};
use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_node::assemble::{Config, Running, assemble};

fn node(config: Config) -> Running {
    let store = Arc::new(Emulator::new(1 << 28, 9));
    let budget = budget(256 << 20, 16 << 20);
    assemble(
        &config,
        Arc::clone(&store) as Arc<dyn BlobStore>,
        &budget,
        writer(&store),
    )
    .expect("assembles")
    .start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        &budget,
    )
    .expect("starts")
}

fn with_roots(fs: &str, s3: &str) -> Config {
    Config {
        replication_remote_roots: (fs.to_owned(), s3.to_owned()),
        stock_s3: Some(oss_node::stock_s3::SourceS3 {
            endpoint: "http://127.0.0.1:1".into(),
            region: "r".into(),
            credentials: oss_blob::Credentials::new("a", "b"),
            own_bucket: "mine".into(),
        }),
        ..config()
    }
}

fn fs_body(alias: &str, root: &str) -> String {
    format!(
        r#"{{"persistent":{{"cluster":{{"remote":{{"{alias}":{{"store":{{"type":"fs","root":"{root}"}}}}}}}}}}}}"#
    )
}

fn put(r: &Running, body: &str) -> support::Reply {
    call(r, "PUT", "/_cluster/settings", Some(body))
}

fn dirs(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let base = std::env::temp_dir().join(format!("oss-remotes-{tag}-{}", std::process::id()));
    let leader = base.join("leader");
    let outside = base.join("outside");
    std::fs::create_dir_all(&leader).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(leader.join("link"));
        std::os::unix::fs::symlink(&outside, leader.join("link")).unwrap();
    }
    (base, leader, outside)
}

#[test]
fn a_remote_under_a_listed_root_is_accepted_and_read_back() {
    let (base, leader, _) = dirs("ok");
    let r = node(with_roots(leader.to_str().unwrap(), ""));
    let ok = put(&r, &fs_body("east", leader.to_str().unwrap()));
    assert_eq!(ok.status, 200, "{}", ok.text());
    let read = call(&r, "GET", "/_cluster/settings?flat_settings=true", None);
    assert!(
        read.text()
            .contains(r#""cluster.remote.east.store.type":"fs""#),
        "{}",
        read.text()
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_remote_outside_every_root_over_a_symlink_or_with_no_roots_is_refused_naming_the_rule() {
    let (base, leader, outside) = dirs("refused");
    let r = node(with_roots(leader.to_str().unwrap(), ""));
    for target in [
        outside.to_str().unwrap().to_owned(),
        leader.join("link").to_str().unwrap().to_owned(),
    ] {
        let refused = put(&r, &fs_body("east", &target));
        assert_eq!(refused.status, 400, "{target}: {}", refused.text());
        assert!(
            refused.text().contains("OSS_REPLICATION_REMOTE_ROOTS"),
            "{}",
            refused.text()
        );
    }
    let none = node(with_roots("", ""));
    let refused = put(&none, &fs_body("east", leader.to_str().unwrap()));
    assert_eq!(refused.status, 400, "{}", refused.text());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn an_s3_remote_over_this_deployments_own_bucket_is_refused_and_one_beside_it_is_not() {
    let r = node(with_roots("", "mine,shared/leaders"));
    let s3 = |bucket: &str, prefix: &str| {
        format!(
            r#"{{"persistent":{{"cluster.remote.w.store.type":"s3","cluster.remote.w.store.bucket":"{bucket}","cluster.remote.w.store.prefix":"{prefix}"}}}}"#
        )
    };
    assert_eq!(put(&r, &s3("mine", "")).status, 400);
    assert_eq!(put(&r, &s3("elsewhere", "x")).status, 400);
    let ok = put(&r, &s3("shared", "leaders/east"));
    assert_eq!(ok.status, 200, "{}", ok.text());
}

#[test]
fn seeds_are_accepted_ignored_and_warned_about() {
    let r = node(with_roots("", ""));
    let body = r#"{"persistent":{"cluster":{"remote":{"leader":{"seeds":["10.0.0.1:9300"]}}}}}"#;
    let ok = put(&r, body);
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert!(
        ok.header("warning").is_some_and(|w| w.contains("seeds")),
        "{:?}",
        ok.headers
    );
    let read = call(&r, "GET", "/_cluster/settings?flat_settings=true", None);
    assert!(!read.text().contains("seeds"), "{}", read.text());
}

#[test]
fn a_bad_type_alias_or_count_is_400() {
    let r = node(with_roots("/", ""));
    let bad_type = r#"{"persistent":{"cluster.remote.a.store.type":"gcs"}}"#;
    assert_eq!(put(&r, bad_type).status, 400);
    let no_bucket = r#"{"persistent":{"cluster.remote.a.store.type":"s3"}}"#;
    assert_eq!(put(&r, no_bucket).status, 400);
    assert_eq!(put(&r, &fs_body("Bad Alias", "/")).status, 400);
    let many: Vec<String> = (0..17)
        .map(|i| {
            format!(r#""cluster.remote.r{i}.store.type":"fs","cluster.remote.r{i}.store.root":"/""#)
        })
        .collect();
    let body = format!(r#"{{"persistent":{{{}}}}}"#, many.join(","));
    assert_eq!(put(&r, &body).status, 400);
}

#[test]
fn the_replication_switch_validates_and_changes_nothing_else() {
    let r = node(with_roots("", ""));
    let set = |v: &str| {
        put(
            &r,
            &format!(r#"{{"persistent":{{"serverless.replication.enabled":"{v}"}}}}"#),
        )
    };
    assert_eq!(set("false").status, 200);
    assert_eq!(set("true").status, 200);
    assert_eq!(set("maybe").status, 400);
    // The settings that exist today still behave as before.
    let ok = put(&r, r#"{"persistent":{"serverless.bucket":"auto"}}"#);
    assert_eq!(ok.status, 200, "{}", ok.text());
}
