//! The scripted update's script (docs/specs/coordinator-update/SPEC.md). Its
//! own test binary: the native-compile switch is process-wide.

use oss_coordinator::docs::update::ScriptOp;
use oss_coordinator::docs::update_script::run_update;
use oss_doc::mapping::json::parse;

fn go(src: &str, found: bool, doc: &str) -> Result<(ScriptOp, String), String> {
    let params = parse(br#"{"n":2}"#).unwrap();
    run_update(src, &params, found, "i", "1", 3, doc.as_bytes())
        .map(|o| (o.op, String::from_utf8(o.source).unwrap()))
}

#[test]
fn an_update_script_edits_the_source_and_chooses_the_operation() {
    oss_reader::painless_switch::set_native_compile(false);
    assert!(
        go("ctx._source.a = 1", true, "{}")
            .unwrap_err()
            .contains("native_compile")
    );

    oss_reader::painless_switch::set_native_compile(true);
    let edit = "ctx._source.count = ctx._source.count + params.n; ctx._source.id = ctx._id";
    assert_eq!(
        go(edit, true, r#"{"count":1}"#).unwrap(),
        (ScriptOp::Index, r#"{"count":3,"id":"1"}"#.to_owned())
    );
    assert_eq!(
        go("ctx.op = 'delete'", true, "{}").unwrap().0,
        ScriptOp::Delete
    );
    assert_eq!(go("ctx.op = 'noop'", true, "{}").unwrap().0, ScriptOp::Noop);
    assert_eq!(
        go("ctx.op = 'whatever'", true, "{}").unwrap().0,
        ScriptOp::Noop
    );
    // a missing document starts as a create over the empty source
    assert_eq!(
        go("ctx._source.a = 1", false, "{}").unwrap(),
        (ScriptOp::Create, r#"{"a":1}"#.to_owned())
    );
    assert_eq!(
        go("ctx.op = 'delete'", false, "{}").unwrap().0,
        ScriptOp::Noop
    );
    assert!(
        go("ctx._source.a = ", true, "{}")
            .unwrap_err()
            .contains("script_exception")
    );
    assert!(
        go("ctx.remove('_source')", true, "{}")
            .unwrap_err()
            .contains("_source")
    );
    assert!(
        go("1", true, "[]")
            .unwrap_err()
            .contains("not a JSON object")
    );
    oss_reader::painless_switch::set_native_compile(false);
}
