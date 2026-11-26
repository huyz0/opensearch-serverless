//! docs/specs/snapshot-aliases AC-5, AC-6, AC-7: the restore's alias keys
//! and the plan of which alias each restored index joins, before any write.

use oss_coordinator::snapshot_restore_admin::{plan, plan_aliases, render_restored, request};
use oss_format::AliasCapture;

fn asked(body: &str) -> oss_coordinator::snapshot_restore_admin::RestoreRequest {
    request("backups", "daily", Some(body.as_bytes())).unwrap()
}

fn alias(name: &str, write: bool) -> AliasCapture {
    AliasCapture {
        name: name.into(),
        write,
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn the_alias_keys_are_accepted_and_the_policy_stays_refused() {
    let r = asked(
        r#"{"include_aliases":true,"rename_alias_pattern":"a","rename_alias_replacement":"b"}"#,
    );
    assert!(r.include_aliases && r.alias_rename.is_some());
    let r = asked(r#"{"include_aliases":false}"#);
    assert!(!r.include_aliases);
    assert!(!asked("{}").include_aliases);
    let e = request("b", "d", Some(br#"{"alias_write_index_policy":"x"}"#)).unwrap_err();
    assert_eq!((e.status, e.kind), (501, "unsupported_restore"));
    let e = request("b", "d", Some(br#"{"include_aliases":"yes"}"#)).unwrap_err();
    assert_eq!((e.status, e.kind), (400, "illegal_argument_exception"));
}

#[test]
fn one_rename_key_alone_is_ignored() {
    let r = asked(r#"{"include_aliases":true,"rename_alias_pattern":"a"}"#);
    assert!(r.alias_rename.is_none());
    let r = asked(r#"{"rename_alias_replacement":"b"}"#);
    assert!(r.alias_rename.is_none());
}

#[test]
fn indices_in_one_alias_join_it_together_in_plan_order() {
    let r = asked(r#"{"include_aliases":true}"#);
    let captured = names(&["a", "b"]);
    let targets = plan(&r, &captured).unwrap();
    let a = [alias("shared", true), alias("only-a", false)];
    let b = [alias("shared", true)];
    let got = plan_aliases(&r, &targets, &[&a, &b]).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].name, "shared");
    assert_eq!(
        got[0].members,
        [("a".to_owned(), true), ("b".to_owned(), true)]
    );
    assert_eq!(got[1].name, "only-a");
    assert_eq!(got[1].members, [("a".to_owned(), false)]);
}

#[test]
fn without_include_aliases_nothing_is_planned() {
    let r = asked(r#"{"rename_alias_pattern":"x","rename_alias_replacement":"y"}"#);
    let targets = plan(&r, &names(&["a"])).unwrap();
    let a = [alias("x1", false)];
    assert!(plan_aliases(&r, &targets, &[&a]).unwrap().is_empty());
}

#[test]
fn a_rename_applies_to_alias_names_and_can_merge_two_into_one() {
    let r = asked(
        r#"{"include_aliases":true,"rename_alias_pattern":"^(.).*","rename_alias_replacement":"$1"}"#,
    );
    let targets = plan(&r, &names(&["a", "b"])).unwrap();
    let a = [alias("xa", true)];
    let b = [alias("xb", true)];
    let got = plan_aliases(&r, &targets, &[&a, &b]).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "x");
    assert_eq!(got[0].members.len(), 2);
}

#[test]
fn an_alias_named_like_a_restored_index_or_not_a_name_is_refused_before_a_write() {
    let r = asked(r#"{"include_aliases":true}"#);
    let targets = plan(&r, &names(&["a", "b"])).unwrap();
    let clash = [alias("b", false)];
    let e = plan_aliases(&r, &targets, &[&clash, &[]]).unwrap_err();
    assert_eq!((e.status, e.kind), (400, "invalid_alias_name_exception"));
    let reserved = [alias("_all", false)];
    assert!(plan_aliases(&r, &targets, &[&reserved, &[]]).is_err());
    let long = asked(
        r#"{"include_aliases":true,"rename_alias_pattern":"^x$","rename_alias_replacement":"yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"}"#,
    );
    let t = plan(&long, &names(&["a"])).unwrap();
    let x = [alias("x", false)];
    assert!(plan_aliases(&long, &t, &[&x]).is_err());
}

#[test]
fn the_distinct_aliases_of_a_request_are_bounded() {
    let r = asked(r#"{"include_aliases":true}"#);
    let captured: Vec<String> = (0..100).map(|i| format!("i{i}")).collect();
    let targets = plan(&r, &captured).unwrap();
    let many = |from: usize, n: usize| -> Vec<AliasCapture> {
        (from..from + n)
            .map(|i| alias(&format!("al{i}"), false))
            .collect()
    };
    let mut lists: Vec<Vec<AliasCapture>> = (0..100).map(|i| many(i * 64, 64)).collect();
    let at_bound: Vec<&[AliasCapture]> = lists.iter().map(Vec::as_slice).collect();
    assert_eq!(plan_aliases(&r, &targets, &at_bound).unwrap().len(), 6_400);
    lists[99].push(alias("one-more", false));
    let over: Vec<&[AliasCapture]> = lists.iter().map(Vec::as_slice).collect();
    let e = plan_aliases(&r, &targets, &over).unwrap_err();
    assert_eq!(e.status, 400);
}

#[test]
fn the_answer_carries_the_alias_report_inside_the_snapshot_object() {
    use oss_coordinator::snapshot_restore_admin::{AliasReport, Target, render_restored_with};
    let targets = [Target {
        source: "a".into(),
        target: "b".into(),
    }];
    let plain = render_restored_with("s", &targets, 2, None);
    assert!(!plain.contains("aliases"), "{plain}");
    let report = AliasReport {
        restored: 3,
        failed: vec!["x".into()],
        truncated: vec!["b".into()],
    };
    let got = render_restored_with("s", &targets, 2, Some(&report));
    assert_eq!(
        got,
        r#"{"snapshot":{"snapshot":"s","indices":["b"],"shards":{"total":2,"failed":0,"successful":2},"aliases_restored":3,"failed_aliases":["x"],"aliases_truncated":["b"]}}"#
    );
    let clean = AliasReport {
        restored: 0,
        failed: vec![],
        truncated: vec![],
    };
    let got = render_restored_with("s", &targets, 2, Some(&clean));
    assert!(got.ends_with(r#""aliases_restored":0}}"#), "{got}");
    assert_eq!(render_restored("s", &targets, 2), plain);
}
