//! The plugin manifest (docs/specs/plugin-system AC-1, AC-2, AC-3, AC-4).

use oss_ext::manifest::{Manifest, ManifestError};
use oss_ext::{Capability, Tier};

const GOOD: &str = r#"{"plugin":{"id":"geo","version":"1.2.0","api":"1.0"},
"points":[{"name":"rest","version":1},{"name":"lifecycle","version":1}],
"routes":[{"method":"GET","path":"/_plugins/geo/{place}","action":"read"}],
"events":["index.created","index.deleted"],
"capabilities":["state.read","state.write","egress:api.example.com","identity"],
"budget":{"state_ops":2,"searches":0,"egress":1},
"limits":{"fuel":1000,"memory_bytes":1048576,"deadline_ms":50}}"#;

fn parse(text: &str) -> Result<Manifest, ManifestError> {
    Manifest::parse(text.as_bytes())
}

fn key_of(text: &str) -> String {
    match parse(text).expect_err("refused") {
        ManifestError::Invalid(e) => e.key().to_owned(),
        other => panic!("not a structural refusal: {other:?}"),
    }
}

// docs/specs/plugin-system AC-1
#[test]
fn a_well_formed_manifest_is_read_whole() {
    let m = parse(GOOD).expect("parses");
    assert_eq!(m.id(), "geo");
    assert_eq!(m.version(), (1, 2, 0));
    assert_eq!(m.api(), (1, 0));
    assert_eq!(m.points().len(), 2);
    assert_eq!(m.points()[0].name(), "rest");
    assert_eq!(m.routes().len(), 1);
    assert_eq!(m.routes()[0].method(), "GET");
    assert_eq!(m.routes()[0].path(), "/_plugins/geo/{place}");
    assert_eq!(m.routes()[0].action(), "read");
    assert_eq!(m.events(), ["index.created", "index.deleted"]);
    assert_eq!(m.capabilities().len(), 4);
    assert_eq!(
        m.capabilities()[2],
        Capability::Egress("api.example.com".into())
    );
    assert_eq!(m.budget().state_ops, 2);
    assert_eq!(m.budget().egress, 1);
    assert_eq!(m.limits().fuel, Some(1000));
}

// docs/specs/plugin-system AC-1
#[test]
fn optional_sections_default_to_nothing_and_a_zero_budget() {
    let m = parse(
        r#"{"plugin":{"id":"echo","version":"0.0.1","api":"1.0"},"points":[{"name":"rest","version":1}]}"#,
    )
    .expect("parses");
    assert!(m.routes().is_empty() && m.events().is_empty() && m.capabilities().is_empty());
    assert_eq!(
        (m.budget().state_ops, m.budget().searches, m.budget().egress),
        (0, 0, 0)
    );
}

// docs/specs/plugin-system AC-1
#[test]
fn an_unknown_duplicate_or_mistyped_key_is_refused_naming_the_key() {
    assert_eq!(key_of(&GOOD.replace(r#""events""#, r#""evens""#)), "evens");
    assert_eq!(
        key_of(&GOOD.replace(r#""api":"1.0""#, r#""api":"1.0","x":1"#)),
        "plugin.x"
    );
    assert_eq!(
        key_of(&GOOD.replace(r#""events""#, r#""events":[],"events""#)),
        "events"
    );
    assert_eq!(
        key_of(&GOOD.replace(r#""version":"1.2.0""#, r#""version":2"#)),
        "plugin.version"
    );
    assert_eq!(
        key_of(&GOOD.replace(r#""state_ops":2"#, r#""state_ops":"2""#)),
        "budget.state_ops"
    );
    assert_eq!(
        key_of(&GOOD.replace(r#""method":"GET""#, r#""method":"GET","y":1"#)),
        "routes[0].y"
    );
    assert_eq!(key_of("[]"), "");
    assert_eq!(key_of("{"), "");
}

// docs/specs/plugin-system AC-1
#[test]
fn an_id_outside_the_pattern_is_refused() {
    for bad in ["", "a", "Geo", "1geo", "geo plugin", "g/o", &"a".repeat(42)] {
        let text = GOOD.replace(r#""id":"geo""#, &format!(r#""id":"{bad}""#));
        assert_eq!(key_of(&text), "plugin.id", "{bad:?}");
    }
    for ok in ["ab", "geo-x_1", &"a".repeat(41)] {
        let text = GOOD.replace(r#""id":"geo""#, &format!(r#""id":"{ok}""#));
        assert!(parse(&text).is_ok(), "{ok:?}");
    }
}

// docs/specs/plugin-system AC-1
#[test]
fn a_manifest_over_the_ceiling_is_refused_whole() {
    let big = format!(
        r#"{{"plugin":{{"id":"geo","version":"1.0.0","api":"1.0"}},"points":[{{"name":"rest","version":1}}],"pad":"{}"}}"#,
        "x".repeat(oss_ext::MAX_DESCRIPTOR_BYTES as usize)
    );
    match parse(&big) {
        Err(ManifestError::Invalid(e)) => assert_eq!(e.key(), ""),
        other => panic!("{other:?}"),
    }
}

// docs/specs/plugin-system AC-2
#[test]
fn the_api_rule_is_the_abis_and_is_judged_before_any_other_key() {
    assert!(parse(GOOD).is_ok());
    // A newer minor may carry keys this host does not know; it is refused for the minor.
    let newer = GOOD
        .replace(r#""api":"1.0""#, r#""api":"1.1""#)
        .replace(r#""events""#, r#""future":1,"events""#);
    assert!(matches!(
        parse(&newer),
        Err(ManifestError::Api(oss_ext::VersionError::NewerMinor {
            module: 1,
            host: 0
        }))
    ));
    let other = GOOD.replace(r#""api":"1.0""#, r#""api":"2.0""#);
    assert!(matches!(
        parse(&other),
        Err(ManifestError::Api(oss_ext::VersionError::OtherMajor { .. }))
    ));
    assert_eq!(oss_ext::manifest::PLUGIN_API, (1, 0));
}

// docs/specs/plugin-system AC-3
#[test]
fn a_point_must_be_in_the_catalogue_at_its_version() {
    assert_eq!(
        key_of(&GOOD.replace(r#""name":"lifecycle""#, r#""name":"nope""#)),
        "points[1].name"
    );
    assert_eq!(
        key_of(&GOOD.replace(
            r#""name":"lifecycle","version":1"#,
            r#""name":"lifecycle","version":2"#
        )),
        "points[1].version"
    );
    assert_eq!(
        key_of(&GOOD.replace(
            r#"{"name":"lifecycle","version":1}"#,
            r#"{"name":"rest","version":1}"#
        )),
        "points[1].name"
    );
}

// docs/specs/plugin-system AC-3
#[test]
fn routes_need_the_rest_point_and_events_the_lifecycle_point() {
    let no_rest = GOOD.replace(r#"{"name":"rest","version":1},"#, "");
    assert_eq!(key_of(&no_rest), "routes");
    let no_life = GOOD.replace(r#",{"name":"lifecycle","version":1}"#, "");
    assert_eq!(key_of(&no_life), "events");
    assert_eq!(
        key_of(&GOOD.replace(r#""index.created""#, r#""index.exploded""#)),
        "events[0]"
    );
}

// docs/specs/plugin-system AC-1
#[test]
fn route_paths_methods_and_actions_are_checked() {
    for bad in [
        "",
        "no-slash",
        "/a//b",
        "/a/../b",
        "/a/{}",
        "/a/{x",
        "/a/b c",
        "/a/{x}{y}",
    ] {
        let text = GOOD.replace("/_plugins/geo/{place}", bad);
        assert_eq!(key_of(&text), "routes[0].path", "{bad:?}");
    }
    assert_eq!(
        key_of(&GOOD.replace(r#""GET""#, r#""TRACE""#)),
        "routes[0].method"
    );
    assert_eq!(
        key_of(&GOOD.replace(r#""read""#, r#""Read All""#)),
        "routes[0].action"
    );
}

// docs/specs/plugin-system AC-4
#[test]
fn capabilities_are_the_known_names_and_an_egress_host_is_plain() {
    for bad in [
        "state",
        "egress:",
        "egress:*.x.com",
        "egress:a b",
        "root",
        "search ",
    ] {
        let text = GOOD.replace(r#""identity""#, &format!(r#""{bad}""#));
        assert_eq!(key_of(&text), "capabilities[3]", "{bad:?}");
    }
    assert_eq!(
        key_of(&GOOD.replace(r#""identity""#, r#""state.read""#)),
        "capabilities[3]"
    );
}

// docs/specs/plugin-system AC-14
#[test]
fn a_budget_is_whole_and_at_most_sixty_four() {
    assert!(parse(&GOOD.replace(r#""state_ops":2"#, r#""state_ops":64"#)).is_ok());
    assert_eq!(
        key_of(&GOOD.replace(r#""state_ops":2"#, r#""state_ops":65"#)),
        "budget.state_ops"
    );
    assert_eq!(
        key_of(&GOOD.replace(r#""searches":0"#, r#""searches":-1"#)),
        "budget.searches"
    );
}

// docs/specs/plugin-system AC-3
#[test]
fn the_tier_matrix_refuses_a_point_the_catalogue_does_not_allow_there() {
    let analyzer = r#"{"plugin":{"id":"tok","version":"1.0.0","api":"1.0"},"points":[{"name":"analyzer","version":1}]}"#;
    let m = parse(analyzer).expect("parses");
    assert!(m.for_tier(Tier::Wasm).is_ok());
    assert!(m.for_tier(Tier::Native).is_ok());
    let e = m
        .for_tier(Tier::Grpc)
        .expect_err("analyzer is not served by grpc");
    assert_eq!(e.key(), "points[0].name");
    assert!(e.reason().contains("grpc"), "{}", e.reason());

    let script = analyzer.replace("analyzer", "script");
    assert!(parse(&script).unwrap().for_tier(Tier::Grpc).is_err());
    let agg = analyzer.replace("analyzer", "agg");
    let agg = parse(&agg).unwrap();
    assert!(agg.for_tier(Tier::Native).is_ok());
    assert!(agg.for_tier(Tier::Wasm).is_err() && agg.for_tier(Tier::Grpc).is_err());
    for point in ["rest", "lifecycle", "ingest", "query", "pipeline_agg"] {
        let m = parse(&analyzer.replace("analyzer", point)).expect(point);
        for t in [Tier::Native, Tier::Wasm, Tier::Grpc] {
            assert!(m.for_tier(t).is_ok(), "{point} on {t:?}");
        }
    }
}

// docs/specs/plugin-system AC-3, AC-4
#[test]
fn a_pure_point_takes_no_capability_and_a_grpc_plugin_only_identity() {
    let pure = r#"{"plugin":{"id":"tok","version":"1.0.0","api":"1.0"},"points":[{"name":"analyzer","version":1}],"capabilities":["log"]}"#;
    let e = parse(pure).unwrap().for_tier(Tier::Wasm).expect_err("pure");
    assert_eq!(e.key(), "capabilities");

    let rest = r#"{"plugin":{"id":"svc","version":"1.0.0","api":"1.0"},"points":[{"name":"rest","version":1}],"capabilities":["CAP"]}"#;
    for cap in [
        "state.read",
        "state.write",
        "search",
        "egress:a.example.com",
        "log",
        "metrics",
    ] {
        let m = parse(&rest.replace("CAP", cap)).expect(cap);
        assert!(m.for_tier(Tier::Native).is_ok() || cap == "log" || cap == "metrics");
        assert_eq!(
            m.for_tier(Tier::Grpc).expect_err(cap).key(),
            "capabilities[0]",
            "{cap}"
        );
    }
    assert!(
        parse(&rest.replace("CAP", "identity"))
            .unwrap()
            .for_tier(Tier::Grpc)
            .is_ok()
    );
}

// docs/specs/plugin-system AC-1
#[test]
fn a_manifest_is_judged_the_same_each_time() {
    assert_eq!(parse(GOOD), parse(GOOD));
}

// docs/specs/plugin-system AC-3
#[test]
fn the_catalogue_names_each_point_tier_and_ceiling() {
    for t in [Tier::Native, Tier::Wasm, Tier::Grpc] {
        assert_eq!(Tier::parse(t.name()), Some(t));
    }
    assert_eq!(Tier::parse("java"), None);
    assert!(oss_ext::point("nope").is_none());
    let rest = oss_ext::point("rest").expect("rest");
    assert_eq!(
        (rest.version, rest.max_in, rest.max_out),
        (1, 1 << 20, 4 << 20)
    );
    assert_eq!(rest.determinism, oss_ext::Determinism::Effectful);
    assert_eq!(
        oss_ext::point("analyzer").unwrap().determinism,
        oss_ext::Determinism::Pure
    );
    assert_eq!(oss_ext::POINTS.len(), 8);
}

// docs/specs/plugin-system AC-1
#[test]
fn shapes_that_are_not_what_a_section_needs_are_refused_naming_it() {
    for (from, to, key) in [
        (r#""routes":["#, r#""routes":{"#, "routes"),
        (
            r#""events":["index.created","index.deleted"]"#,
            r#""events":["index.created","index.created"]"#,
            "events[1]",
        ),
        (
            r#""events":["index.created","index.deleted"]"#,
            r#""events":[3]"#,
            "events[0]",
        ),
        (
            r#""capabilities":["state.read","state.write","#,
            r#""capabilities":["state.read","state.read","#,
            "capabilities[1]",
        ),
        (
            r#""budget":{"state_ops":2,"searches":0,"egress":1}"#,
            r#""budget":[]"#,
            "budget",
        ),
        (
            r#""limits":{"fuel":1000,"memory_bytes":1048576,"deadline_ms":50}"#,
            r#""limits":{"fuel":0}"#,
            "limits.fuel",
        ),
        (
            r#""points":[{"name":"rest","version":1},{"name":"lifecycle","version":1}]"#,
            r#""points":[]"#,
            "points",
        ),
        (
            r#""points":[{"name":"rest","version":1},{"name":"lifecycle","version":1}]"#,
            r#""points":{}"#,
            "points",
        ),
        (
            r#""points":[{"name":"rest","version":1},{"name":"lifecycle","version":1}]"#,
            r#""points":[1]"#,
            "points[0]",
        ),
    ] {
        if from.ends_with('[') {
            let text = GOOD.replace(
                r#""routes":[{"method":"GET","path":"/_plugins/geo/{place}","action":"read"}]"#,
                r#""routes":{}"#,
            );
            assert_eq!(key_of(&text), key);
        } else {
            assert_eq!(key_of(&GOOD.replace(from, to)), key, "{to}");
        }
    }
}

// docs/specs/plugin-system AC-1
#[test]
fn counts_are_bounded_and_bytes_must_be_text() {
    let many = (0..65)
        .map(|_| r#"{"method":"GET","path":"/x","action":"a"}"#)
        .collect::<Vec<_>>()
        .join(",");
    let text = GOOD.replace(
        r#"{"method":"GET","path":"/_plugins/geo/{place}","action":"read"}"#,
        &many,
    );
    assert_eq!(key_of(&text), "routes");
    let caps = (0..17)
        .map(|i| format!(r#""egress:h{i}.example.com""#))
        .collect::<Vec<_>>()
        .join(",");
    let text = GOOD.replace(
        r#""state.read","state.write","egress:api.example.com","identity""#,
        &caps,
    );
    assert_eq!(key_of(&text), "capabilities");
    match Manifest::parse(&[0xff, 0xfe]) {
        Err(ManifestError::Invalid(e)) => assert_eq!(e.key(), ""),
        other => panic!("{other:?}"),
    }
    assert!(parse(&GOOD.replace("/_plugins/geo/{place}", "/")).is_ok());
    assert_eq!(
        key_of(&GOOD.replace(r#""plugin":{"#, r#""plugin":[],"x":{"#)),
        "x"
    );
}
