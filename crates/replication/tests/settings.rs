//! docs/specs/replication/SPEC.md AC-1 and AC-5: what a remote setting may say.

use oss_blob::SourceRoots;
use oss_replication::{Location, remotes};

fn s(pairs: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), Some((*v).to_owned())))
        .collect()
}

#[test]
fn an_s3_store_is_read_back_as_a_remote() {
    let got = remotes(&s(&[
        ("cluster.remote.east.store.type", "s3"),
        ("cluster.remote.east.store.bucket", "leader"),
        ("cluster.remote.east.store.prefix", "prod/"),
    ]))
    .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].alias, "east");
    assert_eq!(
        got[0].location,
        Location::S3 {
            bucket: "leader".into(),
            prefix: "prod/".into()
        }
    );
}

#[test]
fn an_fs_store_needs_a_root_and_an_s3_store_a_bucket() {
    let fs = remotes(&s(&[
        ("cluster.remote.a.store.type", "fs"),
        ("cluster.remote.a.store.root", "/srv/leader"),
    ]))
    .unwrap();
    assert_eq!(
        fs[0].location,
        Location::Fs {
            root: "/srv/leader".into()
        }
    );
    let no_root = remotes(&s(&[("cluster.remote.a.store.type", "fs")])).unwrap_err();
    assert!(no_root.to_string().contains("root"), "{no_root}");
    let no_bucket = remotes(&s(&[("cluster.remote.a.store.type", "s3")])).unwrap_err();
    assert!(no_bucket.to_string().contains("bucket"), "{no_bucket}");
}

#[test]
fn an_unknown_type_a_bad_alias_a_profile_and_a_stray_key_are_refused() {
    for bad in [
        s(&[("cluster.remote.a.store.type", "gcs")]),
        s(&[
            ("cluster.remote.Bad Name.store.type", "fs"),
            ("cluster.remote.Bad Name.store.root", "/x"),
        ]),
        s(&[
            ("cluster.remote.a.store.type", "fs"),
            ("cluster.remote.a.store.root", "/x"),
            ("cluster.remote.a.store.profile", "p"),
        ]),
        s(&[
            ("cluster.remote.a.store.type", "fs"),
            ("cluster.remote.a.store.root", "/x"),
            ("cluster.remote.a.store.colour", "red"),
        ]),
    ] {
        assert!(remotes(&bad).is_err(), "{bad:?}");
    }
}

#[test]
fn seventeen_remotes_are_refused_and_sixteen_are_not() {
    let body = |n: usize| {
        let mut out = Vec::new();
        for i in 0..n {
            out.push((
                format!("cluster.remote.r{i}.store.type"),
                Some("fs".to_owned()),
            ));
            out.push((
                format!("cluster.remote.r{i}.store.root"),
                Some("/x".to_owned()),
            ));
        }
        out
    };
    assert_eq!(remotes(&body(16)).unwrap().len(), 16);
    assert!(remotes(&body(17)).is_err());
}

#[test]
fn removals_and_other_settings_are_not_remotes() {
    let mut set = s(&[("serverless.bucket", "auto")]);
    set.push(("cluster.remote.a.store.type".into(), None));
    assert!(remotes(&set).unwrap().is_empty());
}

#[test]
fn a_remote_is_readable_only_under_a_listed_root_and_never_over_its_own_store() {
    let dir = std::env::temp_dir().join(format!("oss-repl-{}", std::process::id()));
    let leader = dir.join("leader");
    std::fs::create_dir_all(&leader).unwrap();
    let outside = dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let link = dir.join("leader").join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let roots = SourceRoots::parse(leader.to_str().unwrap(), "shared/leaders,other");
    let fs = |p: &std::path::Path| {
        remotes(&s(&[
            ("cluster.remote.a.store.type", "fs"),
            ("cluster.remote.a.store.root", p.to_str().unwrap()),
        ]))
        .unwrap()
        .remove(0)
    };
    let own = Some(("mine", "d/"));
    fs(&leader).check_roots(&roots, own).unwrap();
    assert!(fs(&outside).check_roots(&roots, own).is_err());
    #[cfg(unix)]
    assert!(fs(&link).check_roots(&roots, own).is_err());
    assert!(
        fs(&leader)
            .check_roots(&SourceRoots::default(), own)
            .is_err()
    );

    let s3 = |bucket: &str, prefix: &str| {
        remotes(&s(&[
            ("cluster.remote.a.store.type", "s3"),
            ("cluster.remote.a.store.bucket", bucket),
            ("cluster.remote.a.store.prefix", prefix),
        ]))
        .unwrap()
        .remove(0)
    };
    s3("shared", "leaders/east")
        .check_roots(&roots, own)
        .unwrap();
    assert!(s3("elsewhere", "x").check_roots(&roots, own).is_err());
    // Overlap with this deployment's own bucket and prefix, in both directions.
    let all = SourceRoots::parse("", "mine");
    assert!(s3("mine", "d/sub").check_roots(&all, own).is_err());
    assert!(s3("mine", "").check_roots(&all, own).is_err());
    s3("mine", "elsewhere").check_roots(&all, own).unwrap();
    // A node whose own store is not S3 reads no s3 remote.
    assert!(
        s3("shared", "leaders/east")
            .check_roots(&roots, None)
            .is_err()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
