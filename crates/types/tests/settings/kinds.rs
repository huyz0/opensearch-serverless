//! Each kind parsed as the Java parses it.

use std::collections::{BTreeMap, HashMap};

use oss_types::settings::{
    BoolSetting, ChoiceSetting, Empty, IntSetting, LongSetting, Scope, Source, StrSetting,
};

use crate::{BIG, COUNT, FLAG, MODE, NAME};

fn message<T: std::fmt::Debug>(r: Result<T, oss_types::settings::SettingError>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn a_bool_is_true_or_false_and_nothing_else() {
    assert_eq!(FLAG.parse("true"), Ok(true));
    assert_eq!(FLAG.parse("false"), Ok(false));
    for bad in ["True", "1", "yes", "", " true"] {
        assert_eq!(
            message(FLAG.parse(bad)),
            format!("Failed to parse value [{bad}] as only [true] or [false] are allowed.")
        );
    }
    assert!(FLAG.default());
    assert_eq!(FLAG.key(), "serverless.t.flag");
    assert!(!FLAG.setting().dynamic);
    let err = FLAG.parse("no").unwrap_err();
    assert_eq!(err.key, "serverless.t.flag");
}

#[test]
fn numbers_parse_within_their_bounds_with_the_java_messages() {
    assert_eq!(COUNT.parse("1"), Ok(1));
    assert_eq!(COUNT.parse("64"), Ok(64));
    assert_eq!(COUNT.parse("+7"), Ok(7));
    assert_eq!(
        message(COUNT.parse("0")),
        "Failed to parse value [0] for setting [serverless.t.count] must be >= 1"
    );
    assert_eq!(
        message(COUNT.parse("65")),
        "Failed to parse value [65] for setting [serverless.t.count] must be <= 64"
    );
    for bad in ["x", "1.5", " 3", "99999999999"] {
        assert_eq!(
            message(COUNT.parse(bad)),
            format!("Failed to parse value [{bad}] for setting [serverless.t.count]")
        );
    }
    assert_eq!(COUNT.default(), 8);
    assert_eq!(BIG.parse("-10"), Ok(-10));
    assert_eq!(BIG.parse("99999999999"), Ok(99_999_999_999));
    assert_eq!(
        message(BIG.parse("-11")),
        "Failed to parse value [-11] for setting [serverless.t.big] must be >= -10"
    );
    assert_eq!(BIG.default(), -5);
    let top = LongSetting::new("serverless.t.top", 0, 0, 9, Scope::Node, "d");
    assert_eq!(
        message(top.parse("10")),
        "Failed to parse value [10] for setting [serverless.t.top] must be <= 9"
    );
    assert_eq!(
        message(top.parse("ten")),
        "Failed to parse value [ten] for setting [serverless.t.top]"
    );
}

#[test]
fn a_choice_is_one_of_its_list() {
    assert_eq!(MODE.parse("b"), Ok("b"));
    assert_eq!(
        message(MODE.parse("c")),
        "Failed to parse value [c] for setting [serverless.t.mode]: expected one of [a, b]"
    );
    assert_eq!(MODE.default(), "a");
    assert_eq!(MODE.choices(), ["a", "b"]);
    assert!(MODE.setting().dynamic);
}

#[test]
fn an_absent_key_is_the_default_and_a_present_one_is_never_trimmed() {
    let empty: &[(&str, &str)] = &[];
    assert_eq!(FLAG.get(&empty), Ok(true));
    assert_eq!(COUNT.get(&Empty), Ok(8));
    assert_eq!(BIG.get(&Empty), Ok(-5));
    assert_eq!(NAME.get(&Empty), "dflt");
    assert_eq!(MODE.get(&Empty), Ok("a"));
    let set: &[(&str, &str)] = &[
        ("serverless.t.flag", "false"),
        ("serverless.t.count", " 9"),
        ("serverless.t.big", "3"),
        ("serverless.t.name", " x "),
        ("serverless.t.mode", "b"),
    ];
    assert_eq!(FLAG.get(&set), Ok(false));
    assert!(COUNT.get(&set).is_err());
    assert_eq!(BIG.get(&set), Ok(3));
    assert_eq!(NAME.get(&set), " x ");
    assert_eq!(MODE.get(&set), Ok("b"));
    // Every map is a source.
    let tree: BTreeMap<String, String> = [("serverless.t.count".to_owned(), "2".to_owned())].into();
    let hash: HashMap<String, String> = [("serverless.t.count".to_owned(), "3".to_owned())].into();
    assert_eq!(COUNT.get(&tree), Ok(2));
    assert_eq!(COUNT.get(&hash), Ok(3));
    assert_eq!(Empty.value("serverless.t.count"), None);
}

#[test]
fn a_definition_built_at_run_time_is_the_const_one() {
    // The consts above are built at compile time; these, at run time.
    let key = std::hint::black_box("serverless.t.mode");
    let mode = ChoiceSetting::new(key, "a", &["a", "b"], Scope::Node, "A mode.").dynamic();
    assert_eq!(mode, MODE);
    assert!(
        !ChoiceSetting::new(key, "a", &["a"], Scope::Node, "d")
            .setting()
            .dynamic
    );
    let flag = BoolSetting::new("serverless.t.flag", true, Scope::Node, "A flag.").dynamic();
    assert!(flag.setting().dynamic && !FLAG.setting().dynamic);
    let count = IntSetting::new("serverless.t.count", 8, 1, 64, Scope::Node, "A count.").dynamic();
    assert_eq!(count.setting().key, COUNT.key());
    let big = LongSetting::new(
        "serverless.t.big",
        -5,
        -10,
        i64::MAX,
        Scope::Node,
        "A long.",
    )
    .dynamic();
    assert_eq!(big.default(), BIG.default());
    let name = StrSetting::new("serverless.t.name", "dflt", Scope::Node, "A name.").dynamic();
    assert_eq!(name.default(), NAME.default());
}
