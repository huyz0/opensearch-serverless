//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;
use crate::dsl::parse_chunks;

const MAPPING: &str = r#"{"properties":{
  "title":{"type":"text","fields":{"keyword":{"type":"keyword","ignore_above":5}}},
  "tag":{"type":"keyword"},
  "n":{"type":"integer"},
  "big":{"type":"long"},
  "when":{"type":"date"},
  "obj":{"properties":{"a":{"type":"keyword"},"b":{"type":"short"}}},
  "nv":{"type":"keyword","null_value":"none"},
  "geo":{"type":"geo_point"},
  "copied":{"type":"keyword","copy_to":["target"]},
  "target":{"type":"keyword"}
}}"#;

fn mapping() -> Mapping {
    Mapping::from_json(MAPPING).expect("mapping")
}

fn asked(body: &str) -> Vec<DocValueField> {
    parse_chunks([body.as_bytes()]).expect("body").fields
}

fn resolved(fields: &str) -> Result<Vec<HitField>, ShardError> {
    resolve(&asked(&format!(r#"{{"fields":{fields}}}"#)), &mapping())
}

fn written(fields: &str, source: &str) -> String {
    let hit = resolved(fields).expect("resolves");
    let mut out = Vec::new();
    write_fields(&mut out, &hit, source.as_bytes()).expect("writes");
    String::from_utf8(out).expect("utf-8")
}

fn refusal(fields: &str) -> String {
    match resolved(fields) {
        Err(e) => e.failure.reason,
        Ok(f) => panic!("resolved {f:?}"),
    }
}

#[test]
fn names_and_patterns_resolve_over_the_mapping() {
    let names = |fields: &str| -> Vec<String> {
        let mut n: Vec<String> = resolved(fields)
            .expect("resolves")
            .into_iter()
            .map(|f| f.name)
            .collect();
        n.sort();
        n
    };
    assert_eq!(names(r#"["tag","tag","missing","_id"]"#), ["tag"]);
    assert_eq!(names(r#"["obj.*"]"#), ["obj.a", "obj.b"]);
    assert_eq!(names(r#"["title*"]"#), ["title", "title.keyword"]);
    assert_eq!(names(r#"[{"field":"n"},7]"#), ["n"]);
}

#[test]
fn a_multi_field_reads_its_parents_path() {
    let hit = resolved(r#"["title.keyword","tag"]"#).expect("resolves");
    let path = |name: &str| {
        hit.iter()
            .find(|f| f.name == name)
            .and_then(|f| f.source.as_ref())
            .map(|s| s.path.clone())
    };
    assert_eq!(path("title.keyword").as_deref(), Some("title"));
    assert_eq!(path("tag").as_deref(), Some("tag"));
}

#[test]
fn keyword_and_text_write_the_value_s_text_and_nested_arrays_flatten_breadth_first() {
    let out = written(r#"["tag"]"#, r#"{"tag":["x",["y","z"],"w",null]}"#);
    assert_eq!(
        out,
        r#",\"fields\":{"tag":["x","w","y","z"]}"#.replace('\\', "")
    );
    let out = written(
        r#"["tag"]"#,
        r#"{"tag":[1,2.5,true,-0,10000000000000000000,1e2]}"#,
    );
    assert_eq!(
        out,
        r#","fields":{"tag":["1","2.5","true","0","10000000000000000000","100.0"]}"#
    );
    assert_eq!(
        written(r#"["title"]"#, r#"{"title":"Hello \"W\""}"#),
        r#","fields":{"title":["Hello \"W\""]}"#
    );
}

#[test]
fn a_keyword_over_its_ignore_above_and_a_missing_or_null_value_add_nothing() {
    assert_eq!(
        written(r#"["title.keyword"]"#, r#"{"title":"Hello World"}"#),
        ""
    );
    assert_eq!(
        written(r#"["title.keyword"]"#, r#"{"title":"Hello"}"#),
        r#","fields":{"title.keyword":["Hello"]}"#
    );
    // Six UTF-16 units though five characters: a surrogate pair counts as two.
    assert_eq!(written(r#"["title.keyword"]"#, r#"{"title":"abcd😀"}"#), "");
    assert_eq!(
        written(r#"["title.keyword"]"#, r#"{"title":"abc😀"}"#),
        r#","fields":{"title.keyword":["abc😀"]}"#
    );
    assert_eq!(written(r#"["tag"]"#, r#"{"tag":null}"#), "");
    assert_eq!(written(r#"["tag"]"#, r#"{"other":1}"#), "");
    assert_eq!(written(r#"["tag"]"#, r#"{"tag":[]}"#), "");
}

#[test]
fn numbers_keep_their_precision_and_truncate_a_decimal_part() {
    let out = written(r#"["n","big"]"#, r#"{"n":"7","big":9007199254740993}"#);
    assert!(out.contains(r#""n":[7]"#), "{out}");
    assert!(out.contains(r#""big":[9007199254740993]"#), "{out}");
    assert_eq!(
        written(r#"["n"]"#, r#"{"n":[5.9,-5.9,"6.5",""]}"#),
        r#","fields":{"n":[5,-5,6]}"#
    );
    assert_eq!(
        written(r#"["big"]"#, r#"{"big":[1e3,"12"]}"#),
        r#","fields":{"big":[1000,12]}"#
    );
}

#[test]
fn a_date_is_written_in_the_default_format() {
    assert_eq!(
        written(
            r#"["when"]"#,
            r#"{"when":["2001-03-04","2001-03-04T05:06:07.8Z",983664000123]}"#
        ),
        r#","fields":{"when":["2001-03-04T00:00:00.000Z","2001-03-04T05:06:07.800Z","2001-03-04T00:00:00.123Z"]}"#
    );
}

#[test]
fn a_path_runs_through_objects_arrays_of_objects_and_dotted_keys() {
    let source = r#"{"obj":[{"a":"p"},{"a":["q","r"]},{"c":1},"junk"]}"#;
    assert_eq!(
        written(r#"["obj.a"]"#, source),
        r#","fields":{"obj.a":["p","q","r"]}"#
    );
    assert_eq!(
        written(r#"["obj.a"]"#, r#"{"obj.a":"flat"}"#),
        r#","fields":{"obj.a":["flat"]}"#
    );
    assert_eq!(written(r#"["obj.a"]"#, r#"{"obj":{"a":null}}"#), "");
}

#[test]
fn the_fields_object_is_in_hash_map_order_and_absent_without_a_value() {
    let out = written(
        r#"["n","obj.*","tag"]"#,
        r#"{"tag":"t","n":1,"obj":{"a":"v","b":2}}"#,
    );
    // `HashMap` order of "n", "obj.a", "obj.b", "tag".
    let mut at: Vec<usize> = ["\"n\":", "\"obj.a\":", "\"obj.b\":", "\"tag\":"]
        .iter()
        .map(|k| out.find(k).unwrap_or_else(|| panic!("{k} in {out}")))
        .collect();
    let by_name = at.clone();
    at.sort_unstable();
    let expected: Vec<&str> = {
        let names = ["n", "obj.a", "obj.b", "tag"];
        let mut order = Vec::new();
        java_hash_order(&names, &mut order);
        order
            .iter()
            .filter_map(|&i| names.get(i).copied())
            .collect()
    };
    let in_written: Vec<&str> = {
        let names = ["n", "obj.a", "obj.b", "tag"];
        let mut idx: Vec<usize> = (0..4).collect();
        idx.sort_by_key(|&i| by_name.get(i).copied());
        idx.iter().filter_map(|&i| names.get(i).copied()).collect()
    };
    assert_eq!(in_written, expected);
    assert_eq!(written(r#"["tag"]"#, "{}"), "");
}

#[test]
fn a_format_fails_as_the_mapper_fails() {
    assert_eq!(
        refusal(r#"[{"field":"tag","format":"x"}]"#),
        "Field [tag] of type [keyword] doesn't support formats."
    );
    assert_eq!(
        refusal(r#"[{"field":"n","format":"x"}]"#),
        "Field [n] of type [integer] doesn't support formats."
    );
    assert_eq!(
        refusal(r#"[{"field":"big","format":"x"}]"#),
        "Field [big] of type [long] doesn't support formats."
    );
    assert_eq!(
        refusal(r#"[{"field":"title","format":"x"}]"#),
        "Field [title] doesn't support formats."
    );
    assert!(
        refusal(r#"[{"field":"when","format":"x"}]"#).contains("not supported by this node yet")
    );
}

#[test]
fn what_this_node_does_not_read_is_refused() {
    for fields in [r#"["geo"]"#, r#"["nv"]"#, r#"["target"]"#, r#"["*"]"#] {
        assert!(
            refusal(fields).contains("is not supported by this node yet"),
            "{fields}"
        );
    }
    // The field that copies reads its own path.
    assert!(resolved(r#"["copied"]"#).is_ok());
    assert!(check_alone(true).is_err());
    assert!(check_alone(false).is_ok());
}

#[test]
fn a_value_the_field_cannot_read_fails_the_write() {
    for (fields, source) in [
        (r#"["n"]"#, r#"{"n":"abc"}"#),
        (r#"["n"]"#, r#"{"n":3000000000}"#),
        (r#"["when"]"#, r#"{"when":"yesterday"}"#),
        (r#"["tag"]"#, r#"{"tag":{"a":1}}"#),
        (r#"["tag"]"#, r#"[1]"#),
    ] {
        let hit = resolved(fields).expect("resolves");
        assert!(
            write_fields(&mut Vec::new(), &hit, source.as_bytes()).is_err(),
            "{fields} {source}"
        );
    }
}
