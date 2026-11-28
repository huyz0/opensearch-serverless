//! The policy reader and the registry's refusals
//! (docs/specs/plugin-system AC-3, AC-4, AC-5).

use std::collections::HashMap;
use std::sync::Arc;

use oss_ext::CallError;
use oss_ext::native::NativeInstance;

use super::{Policy, Registry, State, Taken};

struct Fixed(String);

impl NativeInstance for Fixed {
    fn manifest(&self) -> Vec<u8> {
        self.0.clone().into_bytes()
    }

    fn call(&self, _p: &str, input: &[u8], _m: usize) -> Result<Vec<u8>, CallError> {
        Ok(input.to_vec())
    }
}

fn manifest(id: &str, routes: &str, caps: &str) -> String {
    format!(
        r#"{{"plugin":{{"id":"{id}","version":"1.0.0","api":"1.0"}},
"points":[{{"name":"rest","version":1}}],
"routes":[{routes}],"capabilities":[{caps}]}}"#
    )
}

fn route(path: &str) -> String {
    format!(r#"{{"method":"GET","path":"{path}","action":"read"}}"#)
}

fn linked(items: &[(&str, String)]) -> HashMap<String, Arc<dyn NativeInstance>> {
    items
        .iter()
        .map(|(n, m)| {
            (
                (*n).to_owned(),
                Arc::new(Fixed(m.clone())) as Arc<dyn NativeInstance>,
            )
        })
        .collect()
}

type Linked<'a> = &'a [(&'a str, String)];
/// A policy, the plugins linked in, and the words a refusal must hold.
type Case<'a> = (&'a str, Vec<(&'a str, String)>, &'a str);

fn load(policy: &str, items: Linked<'_>, builtin: &[Taken]) -> Registry {
    Registry::load(
        &Policy::parse(policy).expect("policy"),
        &linked(items),
        builtin,
    )
}

fn refusal(r: &Registry, i: usize) -> String {
    match r.plugins()[i].state() {
        State::Refused(why) => why.clone(),
        State::Ready => panic!("plugin {i} is ready"),
    }
}

const ECHO: &str = r#"{"plugins":[{"id":"echo","tier":"native"}]}"#;

#[test]
fn a_policy_names_plugins_and_nothing_else() {
    let p = Policy::parse(
        r#"{"plugins":[{"id":"a","tier":"native","name":"x","grants":["identity","egress:h.io"],
"claim":["/_ltr"],"actions":{"GET /_ltr/x":"read"}}]}"#,
    )
    .expect("parses");
    let e = &p.entries()[0];
    assert_eq!((e.id.as_str(), e.name.as_str()), ("a", "x"));
    assert_eq!(e.grants.len(), 2);
    assert_eq!(e.claims, ["/_ltr"]);
    assert_eq!(e.actions, [("GET /_ltr/x".to_owned(), "read".to_owned())]);
    assert!(Policy::parse("{}").expect("empty").entries().is_empty());
    // The name defaults to the id.
    assert_eq!(Policy::parse(ECHO).expect("ok").entries()[0].name, "echo");
}

#[test]
fn a_policy_that_is_not_exactly_that_is_refused() {
    let bad = [
        "",
        "[]",
        r#"{"plugin":[]}"#,
        r#"{"plugins":{}}"#,
        r#"{"plugins":[{"tier":"native"}]}"#,
        r#"{"plugins":[{"id":"","tier":"native"}]}"#,
        r#"{"plugins":[{"id":"a","tier":"java"}]}"#,
        r#"{"plugins":[{"id":"a","tier":"native","extra":1}]}"#,
        r#"{"plugins":[{"id":"a","tier":"native","grants":["root"]}]}"#,
        r#"{"plugins":[{"id":"a","tier":"native","grants":"identity"}]}"#,
        r#"{"plugins":[{"id":"a","tier":"native","claim":["x"]}]}"#,
        r#"{"plugins":[{"id":"a","tier":"native","claim":["/{a}"]}]}"#,
        r#"{"plugins":[{"id":"a","tier":"native","actions":{"GET /x":1}}]}"#,
        r#"{"plugins":[{"id":"a","tier":"native"},{"id":"a","tier":"native"}]}"#,
        r#"{"plugins":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","tier":"native"}]}"#,
    ];
    for text in bad {
        assert!(Policy::parse(text).is_err(), "{text}");
    }
    let many: Vec<String> = (0..65)
        .map(|i| format!(r#"{{"id":"p{i}","tier":"native"}}"#))
        .collect();
    assert!(Policy::parse(&format!(r#"{{"plugins":[{}]}}"#, many.join(","))).is_err());
    assert!(Policy::parse(&format!(r#"{{"plugins":[{}]}}"#, many[..64].join(","))).is_ok());
}

#[test]
fn a_plugin_that_meets_every_rule_is_ready_with_its_routes() {
    let m = manifest("echo", &route("/_plugins/echo/{word}"), "");
    let r = load(ECHO, &[("echo", m)], &[]);
    let p = &r.plugins()[0];
    assert_eq!(*p.state(), State::Ready);
    assert_eq!(p.manifest().expect("manifest").routes().len(), 1);
    assert!(p.instance().is_some());
}

#[test]
fn each_refusal_names_its_reason_and_keeps_the_node_serving() {
    let ok = manifest("echo", &route("/_plugins/echo/x"), "");
    let cases: [Case; 6] = [
        (
            r#"{"plugins":[{"id":"echo","tier":"wasm"}]}"#,
            vec![],
            "this tier is not available",
        ),
        (ECHO, vec![], "no plugin is registered as `echo`"),
        (ECHO, vec![("echo", "{}".to_owned())], "manifest:"),
        (
            ECHO,
            vec![("echo", manifest("other", &route("/_plugins/other/x"), ""))],
            "the manifest's id is not the policy's",
        ),
        (
            ECHO,
            vec![(
                "echo",
                manifest("echo", &route("/_plugins/echo/x"), r#""identity""#),
            )],
            "asks for identity which the policy does not grant",
        ),
        (
            ECHO,
            vec![("echo", manifest("echo", &route("/_ltr/x"), ""))],
            "outside /_plugins/echo/ and not claimed",
        ),
    ];
    for (policy, items, want) in cases {
        let r = load(policy, &items, &[]);
        assert!(refusal(&r, 0).contains(want), "{want}: {}", refusal(&r, 0));
    }
    // The granted capability and the claimed prefix are accepted.
    let granted =
        r#"{"plugins":[{"id":"echo","tier":"native","grants":["identity"],"claim":["/_ltr"]}]}"#;
    let m = manifest("echo", &route("/_ltr/x"), r#""identity""#);
    assert_eq!(
        *load(granted, &[("echo", m)], &[]).plugins()[0].state(),
        State::Ready
    );
    // A claim is on whole segments: `/_ltrx` is not under `/_ltr`.
    let m = manifest("echo", &route("/_ltrx/x"), "");
    assert!(refusal(&load(granted, &[("echo", m)], &[]), 0).contains("not claimed"));
    let _ = ok;
}

#[test]
fn a_route_that_collides_is_refused_naming_both() {
    let policy = r#"{"plugins":[{"id":"pa","tier":"native","claim":["/_cat"]},{"id":"pb","tier":"native","claim":["/_cat"]}]}"#;
    let builtin = [Taken {
        method: Some("GET".to_owned()),
        pattern: "/_cat/{what}".to_owned(),
        holder: "the built-in".to_owned(),
    }];
    let a = manifest("pa", &route("/_cat/{other}"), "");
    let r = load(policy, &[("pa", a)], &builtin);
    let why = refusal(&r, 0);
    assert!(
        why.contains("GET /_cat/{other} collides with the built-in /_cat/{what}"),
        "{why}"
    );
    // Two plugins on one shape: the second is refused naming the first.
    let a = manifest("pa", &route("/_cat/x/{p}"), "");
    let b = manifest("pb", &route("/_cat/x/{q}"), "");
    let r = load(policy, &[("pa", a), ("pb", b)], &[]);
    assert_eq!(*r.plugins()[0].state(), State::Ready);
    assert!(
        refusal(&r, 1).contains("collides with plugin pa"),
        "{}",
        refusal(&r, 1)
    );
    // A refused plugin holds no route: the next may use its path.
    let bad = manifest("pa", &route("/_cat/x/{p}"), r#""identity""#);
    let b = manifest("pb", &route("/_cat/x/{q}"), "");
    let r = load(policy, &[("pa", bad), ("pb", b)], &[]);
    assert!(matches!(r.plugins()[0].state(), State::Refused(_)));
    assert_eq!(*r.plugins()[1].state(), State::Ready);
    // A different method is not a collision.
    let other = Taken {
        method: Some("POST".to_owned()),
        ..builtin[0].clone()
    };
    let a = manifest("pa", &route("/_cat/{x}"), "");
    assert_eq!(
        *load(policy, &[("pa", a)], &[other]).plugins()[0].state(),
        State::Ready
    );
}

#[test]
fn an_action_row_must_name_a_declared_route() {
    let m = manifest("echo", &route("/_plugins/echo/x"), "");
    let good = r#"{"plugins":[{"id":"echo","tier":"native","actions":{"GET /_plugins/echo/x":"indices:data/read/search"}}]}"#;
    assert_eq!(
        *load(good, &[("echo", m.clone())], &[]).plugins()[0].state(),
        State::Ready
    );
    let bad =
        r#"{"plugins":[{"id":"echo","tier":"native","actions":{"POST /_plugins/echo/x":"a"}}]}"#;
    assert!(refusal(&load(bad, &[("echo", m)], &[]), 0).contains("names no route"));
}
