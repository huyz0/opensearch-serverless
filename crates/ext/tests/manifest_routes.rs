//! A manifest's routes: path parameter names and query parameter lists
//! (docs/specs/plugin-system AC-3, AC-5).

use oss_ext::manifest::{Manifest, ManifestError};

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

// docs/specs/plugin-system AC-3: a path parameter is a lowercase name of letters, digits and underscores.
#[test]
fn path_parameter_names_are_exact() {
    let with = |p: &str| GOOD.replace("{place}", p);
    for ok in ["{a}", "{_a}", "{a1}", "{a_}", "{a_b2}"] {
        assert!(parse(&with(ok)).is_ok(), "{ok} accepted");
    }
    for bad in ["{A}", "{Abc}", "{1a}", "{a-b}", "{aB}", "{}"] {
        assert!(parse(&with(bad)).is_err(), "{bad} refused");
    }
}

// docs/specs/plugin-system AC-5: a route names the query parameters it takes.
#[test]
fn a_route_names_its_query_parameters() {
    let with = |q: &str| {
        GOOD.replace(
            r#""action":"read"}"#,
            &format!(r#""action":"read","query":{q}}}"#),
        )
    };
    let m = parse(&with(r#"["size","from_2"]"#)).expect("parses");
    assert_eq!(m.routes()[0].query(), ["size", "from_2"]);
    assert!(parse(GOOD).expect("none").routes()[0].query().is_empty());
    let sixteen: Vec<String> = (0..16).map(|i| format!("\"p{i}\"")).collect();
    assert!(
        parse(&with(&format!("[{}]", sixteen.join(",")))).is_ok(),
        "sixteen"
    );
    let seventeen: Vec<String> = (0..17).map(|i| format!("\"p{i}\"")).collect();
    assert!(
        parse(&with(&format!("[{}]", seventeen.join(",")))).is_err(),
        "seventeen"
    );
    let forty = "a".repeat(40);
    assert!(
        parse(&with(&format!(r#"["{forty}"]"#))).is_ok(),
        "forty bytes"
    );
    assert!(
        parse(&with(&format!(r#"["{forty}a"]"#))).is_err(),
        "forty-one bytes"
    );
    for bad in [
        r#"[""]"#,
        r#"["Size"]"#,
        r#"["1a"]"#,
        r#"["a-b"]"#,
        r#"["a","a"]"#,
        r#"[1]"#,
        r#""size""#,
    ] {
        assert!(parse(&with(bad)).is_err(), "{bad}");
    }
}

// docs/specs/plugin-system AC-5: a route path is at most 256 bytes, inclusive.
#[test]
fn a_route_path_may_be_exactly_256_bytes() {
    let at = |n: usize| {
        let mut p = String::from("/");
        p.push_str(&"a".repeat(n - 1));
        GOOD.replace("/_plugins/geo/{place}", &p)
    };
    assert!(parse(&at(256)).is_ok(), "256 bytes");
    assert!(parse(&at(257)).is_err(), "257 bytes");
}
