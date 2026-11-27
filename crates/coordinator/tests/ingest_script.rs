//! The ingest `script` processor (docs/specs/ingest-pipelines/SPEC.md). Its own
//! test binary: the native-compile switch is process-wide.

use oss_coordinator::ingest::Pipeline;

fn run(pipeline: &str, source: &str) -> Result<String, String> {
    let p = Pipeline::compile(pipeline.as_bytes())?;
    p.run(source.as_bytes())
        .map(|b| String::from_utf8(b).expect("utf8"))
}

fn script(config: &str, source: &str) -> Result<String, String> {
    run(
        &format!(r#"{{"processors":[{{"script":{config}}}]}}"#),
        source,
    )
}

#[test]
fn the_script_processor_is_refused_until_the_switch_is_on_then_edits_the_document() {
    oss_reader::painless_switch::set_native_compile(false);
    let refused = script(r#"{"source":"ctx.a = 1"}"#, "{}").unwrap_err();
    assert!(refused.contains("[script] is not supported"), "{refused}");

    oss_reader::painless_switch::set_native_compile(true);
    // reads fields, params, nested maps and lists
    let out = script(
        r#"{"source":"ctx.total = ctx.price * params.qty; ctx.tags.add('x'); ctx.meta.seen = true; ctx.remove('old')","params":{"qty":3}}"#,
        r#"{"price":2,"tags":["a"],"meta":{},"old":1}"#,
    )
    .unwrap();
    assert_eq!(
        out,
        r#"{"price":2,"tags":["a","x"],"meta":{"seen":true},"total":6}"#
    );
    // a double stays a double
    assert_eq!(
        script(r#"{"source":"ctx.h = ctx.v / 2.0"}"#, r#"{"v":3}"#).unwrap(),
        r#"{"v":3,"h":1.5}"#
    );

    // options this engine does not run are refused at compile
    for (config, needle) in [
        (
            r#"{"source":"1","lang":"mustache"}"#,
            "only lang [painless]",
        ),
        (r#"{"id":"stored"}"#, "does not support the option [id]"),
        (r#"{"lang":"painless"}"#, "[source] required"),
        (r#"{"source":"1","params":3}"#, "[params] must be an object"),
    ] {
        let e = script(config, "{}").unwrap_err();
        assert!(e.contains(needle), "{config}: {e}");
    }

    // a script that does not compile or throws fails the document, and on_failure takes it
    assert!(
        script(r#"{"source":"ctx.a = "}"#, "{}")
            .unwrap_err()
            .contains("script_exception")
    );
    let thrown = script(
        r#"{"source":"throw new IllegalArgumentException('no')"}"#,
        "{}",
    )
    .unwrap_err();
    assert!(thrown.contains("IllegalArgumentException"), "{thrown}");
    let handled = run(
        r#"{"processors":[{"script":{"source":"ctx.a = ","on_failure":[{"set":{"field":"failed","value":true}}]}}]}"#,
        "{}",
    )
    .unwrap();
    assert_eq!(handled, r#"{"failed":true}"#);
    oss_reader::painless_switch::set_native_compile(false);
}
