//! A WASM module's describe output read as a plugin manifest
//! (docs/specs/plugin-system AC-1): one parser, the same answer.

use oss_ext::Descriptor;
use oss_ext::manifest::{Manifest, ManifestError};

const DESCRIBE: &str = r#"{"name":"geo","version":"1.2.0","abi":"1.0","io_bytes":65536,
"max_concurrent_calls":4,
"limits":{"fuel":1000,"memory_bytes":1048576,"deadline_ms":50},
"points":[{"id":"rest","version":"1.0.0"},{"id":"lifecycle","version":"1.0.0"}],
"plugin":{"api":"1.0",
"routes":[{"method":"GET","path":"/_plugins/geo/{place}","action":"read"}],
"events":["index.created","index.deleted"],
"capabilities":["state.read","state.write","egress:api.example.com","identity"],
"budget":{"state_ops":2,"searches":0,"egress":1}}}"#;

const NATIVE: &str = r#"{"plugin":{"id":"geo","version":"1.2.0","api":"1.0"},
"points":[{"name":"rest","version":1},{"name":"lifecycle","version":1}],
"routes":[{"method":"GET","path":"/_plugins/geo/{place}","action":"read"}],
"events":["index.created","index.deleted"],
"capabilities":["state.read","state.write","egress:api.example.com","identity"],
"budget":{"state_ops":2,"searches":0,"egress":1},
"limits":{"fuel":1000,"memory_bytes":1048576,"deadline_ms":50}}"#;

fn read(text: &str) -> Result<Manifest, ManifestError> {
    let d = Descriptor::parse(text.as_bytes()).map_err(ManifestError::from)?;
    Manifest::from_descriptor(&d)
}

fn invalid_key(text: &str) -> String {
    match read(text) {
        Err(ManifestError::Invalid(e)) => e.key().to_owned(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_describe_output_and_a_native_manifest_say_the_same_thing() {
    let wasm = read(DESCRIBE).expect("reads");
    let native = Manifest::parse(NATIVE.as_bytes()).expect("parses");
    assert_eq!(wasm, native);
}

#[test]
fn a_module_with_no_plugin_section_is_not_a_plugin() {
    let bare = DESCRIBE.replace(r#""plugin":{"api":"1.0","#, r#""x":{"#);
    assert_eq!(
        invalid_key(&bare),
        "x",
        "its own unknown key is refused first"
    );
    let none = r#"{"name":"geo","version":"1.2.0","abi":"1.0","io_bytes":65536,
"max_concurrent_calls":4,"points":[{"id":"rest","version":"1.0.0"}]}"#;
    assert_eq!(invalid_key(none), "plugin");
}

#[test]
fn a_key_the_section_does_not_know_is_refused_by_name() {
    let bad = DESCRIBE.replace(
        r#""plugin":{"api":"1.0","#,
        r#""plugin":{"api":"1.0","extra":1,"#,
    );
    assert_eq!(invalid_key(&bad), "plugin.extra");
    let not_object = DESCRIBE.replace(r#""plugin":{"#, r#""plugin":[{"#);
    assert!(Descriptor::parse(not_object.as_bytes()).is_err());
}

#[test]
fn the_section_must_name_its_api_and_a_newer_minor_is_refused_for_that() {
    let no_api = DESCRIBE.replace(r#""api":"1.0","#, "");
    assert_eq!(invalid_key(&no_api), "plugin.api");
    let newer = DESCRIBE.replace(r#""api":"1.0","#, r#""api":"1.9","#);
    assert!(matches!(read(&newer), Err(ManifestError::Api(_))));
}

#[test]
fn the_same_tier_rules_judge_the_manifest_a_module_delivers() {
    use oss_ext::Tier;
    let m = read(DESCRIBE).expect("reads");
    assert!(m.for_tier(Tier::Wasm).is_ok());
    assert!(
        m.for_tier(Tier::Grpc).is_err(),
        "an out-of-process plugin holds no host capability"
    );
}
