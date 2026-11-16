//! Index over cluster over node over default.

use oss_types::settings::{
    BoolSetting, IntSetting, Layer, Layers, LongSetting, Resolved, Scope, SettingError, Source,
    StrSetting,
};

use crate::{BIG, COUNT, FLAG, INDEX_MODE, MODE, NAME};

fn at<T>(value: T, layer: Layer) -> Result<Resolved<T>, SettingError> {
    Ok(Resolved { value, layer })
}

type Pairs<'a> = &'a [(&'a str, &'a str)];

/// No pairs, as a source the layers can borrow.
const NONE: Pairs<'static> = &[];

fn layers<'a>(
    index: Option<&'a Pairs<'a>>,
    cluster: &'a Pairs<'a>,
    node: &'a Pairs<'a>,
) -> Layers<'a> {
    Layers {
        index: index.map(|i| i as &dyn Source),
        cluster,
        node,
    }
}

#[test]
fn index_overrides_cluster_overrides_node() {
    let index: &[(&str, &str)] = &[("index.serverless.t.mode", "b")];
    let cluster: &[(&str, &str)] = &[("serverless.t.mode", "a"), ("serverless.t.name", "c")];
    let node: &[(&str, &str)] = &[("serverless.t.mode", "b"), ("serverless.t.name", "n")];
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&index), &cluster, &node)),
        at("b", Layer::Index)
    );
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(None, &cluster, &node)),
        at("a", Layer::Cluster)
    );
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&NONE), &NONE, &node)),
        at("b", Layer::Node)
    );
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(None, &NONE, &NONE)),
        at("a", Layer::Default)
    );
    // The index key is the index setting's own; the node key there is not.
    let wrong: &[(&str, &str)] = &[("serverless.t.mode", "b")];
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&wrong), &NONE, &NONE)),
        at("a", Layer::Default)
    );
    let name = StrSetting::new("index.serverless.t.name", "", Scope::Index, "d");
    let r = NAME.resolve(&name, &layers(None, &cluster, &node));
    assert_eq!((r.value, r.layer), ("c", Layer::Cluster));
    let r = NAME.resolve(&name, &layers(None, &NONE, &NONE));
    assert_eq!((r.value, r.layer), ("dflt", Layer::Default));
    let index_flag = BoolSetting::new("index.serverless.t.flag", true, Scope::Index, "d");
    let flags: &[(&str, &str)] = &[("index.serverless.t.flag", "false")];
    assert_eq!(
        FLAG.resolve(&index_flag, &layers(Some(&flags), &NONE, &NONE)),
        at(false, Layer::Index)
    );
    let index_count = IntSetting::new("index.serverless.t.count", 8, 1, 64, Scope::Index, "d");
    let counts: &[(&str, &str)] = &[("serverless.t.count", "5")];
    assert_eq!(
        COUNT.resolve(&index_count, &layers(None, &counts, &NONE)),
        at(5, Layer::Cluster)
    );
    let index_big = LongSetting::new("index.serverless.t.big", 0, -10, 10, Scope::Index, "d");
    let bigs: &[(&str, &str)] = &[("serverless.t.big", "7")];
    assert_eq!(
        BIG.resolve(&index_big, &layers(None, &NONE, &bigs)),
        at(7, Layer::Node)
    );
}

#[test]
fn an_empty_value_is_unset_at_every_layer() {
    let index: &[(&str, &str)] = &[("index.serverless.t.mode", "")];
    let cluster: &[(&str, &str)] = &[("serverless.t.mode", "")];
    let node: &[(&str, &str)] = &[("serverless.t.mode", "b")];
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&index), &cluster, &node)),
        Ok(Resolved {
            value: "b",
            layer: Layer::Node
        })
    );
    let node: &[(&str, &str)] = &[("serverless.t.mode", "")];
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&index), &cluster, &node)),
        Ok(Resolved {
            value: "a",
            layer: Layer::Default
        })
    );
}

#[test]
fn a_bad_value_is_skipped_and_the_next_layer_decides() {
    // The Java's cases (`ServerlessSettingsTests`): a bad index value falls
    // to the cluster's, a bad cluster value to the node's.
    let bad_index: &[(&str, &str)] = &[("index.serverless.t.mode", "z")];
    let cluster: &[(&str, &str)] = &[("serverless.t.mode", "b")];
    let bad_cluster: &[(&str, &str)] = &[("serverless.t.mode", "y")];
    let node: &[(&str, &str)] = &[("serverless.t.mode", "b")];
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&bad_index), &cluster, &NONE)),
        at("b", Layer::Cluster)
    );
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&bad_index), &bad_cluster, &node)),
        at("b", Layer::Node)
    );
    assert_eq!(
        MODE.resolve(&INDEX_MODE, &layers(Some(&bad_index), &bad_cluster, &NONE)),
        at("a", Layer::Default)
    );
    // A bad node value is refused, under the node's key, as the Java
    // refuses it when the node starts.
    let err = MODE
        .resolve(&INDEX_MODE, &layers(None, &NONE, &bad_cluster))
        .unwrap_err();
    assert_eq!(err.key, "serverless.t.mode");
    // Each kind skips: a bool, an int out of the index setting's bounds,
    // and a long.
    let index_flag = BoolSetting::new("index.serverless.t.flag", false, Scope::Index, "d");
    let flags: &[(&str, &str)] = &[
        ("index.serverless.t.flag", "maybe"),
        ("serverless.t.flag", "maybe"),
    ];
    assert!(!index_flag.default());
    assert_eq!(
        FLAG.resolve(&index_flag, &layers(Some(&flags), &flags, &NONE)),
        at(true, Layer::Default)
    );
    let off: &[(&str, &str)] = &[("serverless.t.flag", "false")];
    assert_eq!(
        FLAG.resolve(&index_flag, &layers(Some(&flags), &off, &NONE)),
        at(false, Layer::Cluster)
    );
    let index_count = IntSetting::new("index.serverless.t.count", 8, 1, 2, Scope::Index, "d");
    let counts: &[(&str, &str)] = &[
        ("index.serverless.t.count", "5"),
        ("serverless.t.count", "5"),
    ];
    assert_eq!(
        COUNT.resolve(&index_count, &layers(Some(&counts), &counts, &NONE)),
        at(5, Layer::Cluster)
    );
    let bad_node: &[(&str, &str)] = &[("serverless.t.count", "99")];
    let err = COUNT
        .resolve(&index_count, &layers(None, &NONE, &bad_node))
        .unwrap_err();
    assert_eq!(
        err.message,
        "Failed to parse value [99] for setting [serverless.t.count] must be <= 64"
    );
    let index_big = LongSetting::new("index.serverless.t.big", 0, -1, 1, Scope::Index, "d");
    let bigs: &[(&str, &str)] = &[("index.serverless.t.big", "2"), ("serverless.t.big", "x")];
    assert_eq!(
        BIG.resolve(&index_big, &layers(Some(&bigs), &bigs, &NONE)),
        at(-5, Layer::Default)
    );
}
