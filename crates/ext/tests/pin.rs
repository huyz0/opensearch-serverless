//! An index's analyzer pins, read from create settings
//! (docs/specs/extension-analyzer AC-4, AC-5, AC-7).

use oss_ext::pin::{
    MAX_PIN_SETTINGS_BYTES, MAX_PINS, Pin, PinError, ResolvedLimits, parse_pins, render,
};

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn s(k: &str, v: &str) -> (String, Option<String>) {
    (k.to_owned(), Some(v.to_owned()))
}

fn pin_keys(name: &str, hash: &str, version: &str) -> Vec<(String, Option<String>)> {
    let p = format!("index.analysis.analyzer.{name}");
    vec![
        s(&format!("{p}.type"), "serverless_module"),
        s(&format!("{p}.module"), hash),
        s(&format!("{p}.version"), version),
    ]
}

fn refused(settings: &[(String, Option<String>)]) -> PinError {
    parse_pins(settings).expect_err("refused")
}

#[test]
fn settings_with_no_analysis_keys_have_no_pins() {
    assert_eq!(
        parse_pins(&[s("index.refresh_interval", "1s")]).expect("none"),
        vec![]
    );
}

#[test]
fn a_pin_is_read_from_its_three_keys() {
    let got = parse_pins(&pin_keys("ext_stem", HASH, "1")).expect("parses");
    assert_eq!(
        got,
        vec![Pin {
            name: "ext_stem".into(),
            module: HASH.into(),
            version: 1
        }]
    );
}

#[test]
fn a_name_without_the_prefix_or_with_other_characters_is_refused() {
    for bad in [
        "standard", "default", "keyword", "stem", "ext_", "ext_Up", "ext_a-b",
    ] {
        let e = refused(&pin_keys(bad, HASH, "1"));
        assert!(e.key().contains(bad), "{bad}: {e:?}");
    }
    let long = format!("ext_{}", "a".repeat(61));
    assert!(parse_pins(&pin_keys(&long, HASH, "1")).is_err());
    let longest = format!("ext_{}", "a".repeat(60));
    assert!(parse_pins(&pin_keys(&longest, HASH, "1")).is_ok());
}

#[test]
fn the_type_module_and_version_must_be_there_and_right() {
    let mut no_type = pin_keys("ext_a", HASH, "1");
    no_type.remove(0);
    assert!(refused(&no_type).reason().contains("type"));
    let mut other_type = pin_keys("ext_a", HASH, "1");
    other_type[0].1 = Some("custom".into());
    assert!(refused(&other_type).reason().contains("serverless_module"));
    assert!(
        refused(&pin_keys("ext_a", "ABC", "1"))
            .reason()
            .contains("64 lowercase")
    );
    assert!(
        refused(&pin_keys("ext_a", &HASH.to_uppercase(), "1"))
            .reason()
            .contains("64 lowercase")
    );
    assert!(
        refused(&pin_keys("ext_a", HASH, "2"))
            .reason()
            .contains("version")
    );
    assert!(
        refused(&pin_keys("ext_a", HASH, "one"))
            .reason()
            .contains("version")
    );
    let mut no_module = pin_keys("ext_a", HASH, "1");
    no_module.remove(1);
    assert!(refused(&no_module).reason().contains("module"));
}

#[test]
fn a_client_cannot_name_limits_or_any_other_analysis_key() {
    for key in [
        "index.analysis.analyzer.ext_a.limits.fuel",
        "index.analysis.analyzer.ext_a.tokenizer",
        "index.analysis.filter.f.type",
        "index.analysis.analyzer.default.type",
        "index.analysis.char_filter.c.type",
        "index.analysis.analyzer.ext_a.module.extra",
    ] {
        let mut st = pin_keys("ext_a", HASH, "1");
        st.push(s(key, "1"));
        let e = refused(&st);
        assert_eq!(e.key(), key);
    }
}

#[test]
fn a_null_value_is_refused() {
    let mut st = pin_keys("ext_a", HASH, "1");
    st[1].1 = None;
    assert!(refused(&st).reason().contains("module"));
}

#[test]
fn at_most_four_pins_and_a_bounded_size() {
    let mut st = Vec::new();
    for i in 0..MAX_PINS {
        st.extend(pin_keys(&format!("ext_p{i}"), HASH, "1"));
    }
    assert_eq!(parse_pins(&st).expect("four").len(), MAX_PINS);
    st.extend(pin_keys("ext_p9", HASH, "1"));
    assert!(refused(&st).reason().contains("at most"));
    let mut big = pin_keys("ext_a", HASH, "1");
    big.push(s(
        "index.analysis.analyzer.ext_a.module",
        &"a".repeat(MAX_PIN_SETTINGS_BYTES),
    ));
    assert!(parse_pins(&big).is_err());
}

#[test]
fn rendering_adds_the_resolved_limits_in_a_stable_order() {
    let pin = Pin {
        name: "ext_a".into(),
        module: HASH.into(),
        version: 1,
    };
    let lim = ResolvedLimits {
        fuel: 50_000_000,
        memory_bytes: 67_108_864,
        deadline_ms: 500,
    };
    let got = render(&[(pin, lim)]);
    let want = [
        ("index.analysis.analyzer.ext_a.type", "serverless_module"),
        ("index.analysis.analyzer.ext_a.module", HASH),
        ("index.analysis.analyzer.ext_a.version", "1"),
        ("index.analysis.analyzer.ext_a.limits.fuel", "50000000"),
        (
            "index.analysis.analyzer.ext_a.limits.memory_bytes",
            "67108864",
        ),
        ("index.analysis.analyzer.ext_a.limits.deadline_ms", "500"),
    ];
    let want: Vec<(String, String)> = want
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    assert_eq!(got, want);
}
