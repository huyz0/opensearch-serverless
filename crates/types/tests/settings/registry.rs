//! The registry's rules, cluster updates, and live settings.

use std::sync::Arc;

use oss_types::settings::{
    BoolSetting, ChoiceSetting, IntSetting, Live, LongSetting, Registry, Scope, Setting, Snapshot,
    StrSetting,
};

use crate::{COUNT, FLAG, INDEX_MODE, MODE, NAME};

static PROVIDER: [&Setting; 3] = [FLAG.setting(), COUNT.setting(), MODE.setting()];
static INDEX_PROVIDER: [&Setting; 1] = [INDEX_MODE.setting()];

fn registry() -> Registry {
    Registry::new(&[&PROVIDER, &INDEX_PROVIDER]).unwrap()
}

#[test]
fn a_registry_refuses_a_key_defined_twice() {
    static AGAIN: [&Setting; 1] = [COUNT.setting()];
    let err = Registry::new(&[&PROVIDER, &AGAIN]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "setting [serverless.t.count] is defined twice"
    );
}

#[test]
fn a_registry_refuses_a_key_outside_its_prefix_or_without_a_doc() {
    static NODE_AS_INDEX: BoolSetting = BoolSetting::new("serverless.x", true, Scope::Index, "d");
    static INDEX_AS_NODE: BoolSetting =
        BoolSetting::new("index.serverless.x", true, Scope::Node, "d");
    static BARE: BoolSetting = BoolSetting::new("serverless.", true, Scope::Node, "d");
    static NO_DOC: BoolSetting = BoolSetting::new("serverless.x", true, Scope::Node, "  ");
    for (setting, message) in [
        (
            NODE_AS_INDEX.setting(),
            "setting [serverless.x] is not under [index.serverless.]",
        ),
        (
            INDEX_AS_NODE.setting(),
            "setting [index.serverless.x] is not under [serverless.]",
        ),
        (
            BARE.setting(),
            "setting [serverless.] is not under [serverless.]",
        ),
        (NO_DOC.setting(), "setting [serverless.x] has no doc"),
    ] {
        let one: &[&Setting] = &[setting];
        assert_eq!(Registry::new(&[one]).unwrap_err().to_string(), message);
    }
}

#[test]
fn a_registry_refuses_a_default_it_would_refuse_as_a_value() {
    static LOW: IntSetting = IntSetting::new("serverless.x", 0, 1, 2, Scope::Node, "d");
    static HIGH: IntSetting = IntSetting::new("serverless.x", 3, 1, 2, Scope::Node, "d");
    static LONG_LOW: LongSetting = LongSetting::new("serverless.x", 0, 1, 2, Scope::Node, "d");
    static LONG_HIGH: LongSetting = LongSetting::new("serverless.x", 3, 1, 2, Scope::Node, "d");
    static CHOICE: ChoiceSetting =
        ChoiceSetting::new("serverless.x", "c", &["a"], Scope::Node, "d");
    for setting in [
        LOW.setting(),
        HIGH.setting(),
        LONG_LOW.setting(),
        LONG_HIGH.setting(),
        CHOICE.setting(),
    ] {
        let one: &[&Setting] = &[setting];
        assert_eq!(
            Registry::new(&[one]).unwrap_err().to_string(),
            "setting [serverless.x] has a default it would refuse as a value"
        );
    }
    static EDGES: IntSetting = IntSetting::new("serverless.x", 1, 1, 1, Scope::Node, "d");
    static LONG_EDGES: LongSetting = LongSetting::new("serverless.y", 2, 2, 2, Scope::Node, "d");
    let ok: &[&Setting] = &[EDGES.setting(), LONG_EDGES.setting(), NAME.setting()];
    assert_eq!(Registry::new(&[ok]).unwrap().all().count(), 3);
}

#[test]
fn a_registry_finds_keys_and_reports_unknown_ones() {
    let r = registry();
    assert_eq!(r.get("serverless.t.count"), Some(COUNT.setting()));
    assert_eq!(r.get("serverless.t.counts"), None);
    assert_eq!(r.get("index.serverless.t.mode"), Some(INDEX_MODE.setting()));
    let keys: Vec<_> = r.all().map(|s| s.key).collect();
    assert_eq!(
        keys,
        [
            "index.serverless.t.mode",
            "serverless.t.count",
            "serverless.t.flag",
            "serverless.t.mode"
        ]
    );
    let index: Vec<_> = r.index_scoped().map(|s| s.key).collect();
    assert_eq!(index, ["index.serverless.t.mode"]);
    let given = [
        "serverless.t.zz",
        "serverless.t.count",
        "path.data",
        "serverless.t.aa",
        "serverless.t.zz",
    ];
    assert_eq!(
        r.unknown_node_keys(given),
        ["serverless.t.aa", "serverless.t.zz"]
    );
}

#[test]
fn an_update_is_checked_against_the_registry() {
    let r = registry();
    assert_eq!(r.check_update("serverless.t.mode", Some("b")), Ok(()));
    assert_eq!(r.check_update("serverless.t.mode", None), Ok(()));
    assert_eq!(
        r.check_update("cluster.routing.x", Some("anything")),
        Ok(())
    );
    for (key, value, message) in [
        (
            "serverless.t.nope",
            Some("1"),
            "unknown setting [serverless.t.nope] please check that any required plugins are installed, \
             or check the breaking changes documentation for removed settings",
        ),
        (
            "serverless.t.count",
            Some("9"),
            "persistent setting [serverless.t.count], not dynamically updateable",
        ),
        (
            "serverless.t.mode",
            Some("q"),
            "Failed to parse value [q] for setting [serverless.t.mode]: expected one of [a, b]",
        ),
    ] {
        assert_eq!(message_of(r.check_update(key, value)), message, "{key}");
    }
    // Each kind's value is checked.
    static D_FLAG: BoolSetting =
        BoolSetting::new("serverless.d.flag", true, Scope::Node, "d").dynamic();
    static D_INT: IntSetting =
        IntSetting::new("serverless.d.int", 1, 1, 2, Scope::Node, "d").dynamic();
    static D_LONG: LongSetting =
        LongSetting::new("serverless.d.long", 1, 1, 2, Scope::Node, "d").dynamic();
    static D_STR: StrSetting = StrSetting::new("serverless.d.str", "", Scope::Node, "d").dynamic();
    let dynamic: &[&Setting] = &[
        D_FLAG.setting(),
        D_INT.setting(),
        D_LONG.setting(),
        D_STR.setting(),
    ];
    let r = Registry::new(&[dynamic]).unwrap();
    assert!(r.check_update("serverless.d.flag", Some("x")).is_err());
    assert!(r.check_update("serverless.d.int", Some("3")).is_err());
    assert!(r.check_update("serverless.d.long", Some("0")).is_err());
    assert_eq!(r.check_update("serverless.d.str", Some(" any ")), Ok(()));
    assert_eq!(r.check_update("serverless.d.int", Some("2")), Ok(()));
}

#[test]
fn an_index_setting_is_no_cluster_update() {
    let r = registry();
    assert!(r.index_scoped().all(|s| s.scope == Scope::Index));
    // A defined index setting is unknown to the cluster's settings, as in
    // the Java, and so is any other key under the index prefix.
    for key in ["index.serverless.t.mode", "index.serverless.t.nope"] {
        assert_eq!(
            message_of(r.check_update(key, Some("a"))),
            format!(
                "unknown setting [{key}] please check that any required plugins are installed, \
                 or check the breaking changes documentation for removed settings"
            )
        );
    }
    // Core's keys under `index.` but outside the prefix are core's to check.
    assert_eq!(r.check_update("index.number_of_shards", Some("1")), Ok(()));
}

fn message_of(r: Result<(), oss_types::settings::SettingError>) -> String {
    r.map_or_else(|e| e.message, |()| String::new())
}

#[test]
fn live_settings_take_only_newer_generations() {
    let live = Live::new();
    assert_eq!(live.current().generation, 0);
    let at = |generation: u64, v: &str| Snapshot {
        generation,
        values: [("serverless.t.mode".to_owned(), v.to_owned())].into(),
    };
    assert!(live.replace(at(5, "b")));
    assert_eq!(MODE.get(&*live.current()), Ok("b"));
    assert!(!live.replace(at(5, "a")));
    assert!(!live.replace(at(4, "a")));
    assert_eq!(MODE.get(&*live.current()), Ok("b"));
    assert!(live.replace(at(6, "a")));
    assert_eq!(live.current().generation, 6);
}

#[test]
fn a_snapshot_outlives_the_swap() {
    let live = Arc::new(Live::new());
    let first = Snapshot {
        generation: 1,
        values: [("serverless.t.count".to_owned(), "3".to_owned())].into(),
    };
    live.replace(first);
    let held = live.current();
    live.replace(Snapshot {
        generation: 2,
        ..Snapshot::default()
    });
    assert_eq!(COUNT.get(&*held), Ok(3));
    assert_eq!(COUNT.get(&*live.current()), Ok(8));
    // A reader on another thread sees a whole snapshot.
    let other = Arc::clone(&live);
    let seen = std::thread::spawn(move || other.current().generation)
        .join()
        .unwrap();
    assert_eq!(seen, 2);
}
