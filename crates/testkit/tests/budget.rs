//! docs/specs/request-budgets/SPEC.md AC-1, AC-2: the table parses, and a
//! measurement is held to its row.

use oss_testkit::budget::{self, Bound, Row, Status};

fn row(bound: Bound, budget: u64) -> Row {
    Row {
        key: "x.y".to_owned(),
        what: "a case".to_owned(),
        budget,
        bound,
        status: Status::Enforced,
        gate: "crates/x/tests/y.rs::z".to_owned(),
        line: 1,
    }
}

#[test]
fn the_table_parses_and_keeps_a_blocked_row() {
    let rows = budget::rows();
    assert!(!rows.is_empty());
    assert!(rows.iter().any(|r| r.status == Status::Blocked));
    let parsed = budget::parse(
        "| Objective | Case | Budget | Bound | Status | Gate |\n\
         |---|---|---|---|---|---|\n\
         text that is no row\n\
         | `a.b_2` | fine | 1,200 | max | enforced | `crates/x/tests/y.rs::z` |\n\
         | `a.c` | waits | 0 | exact | blocked | something |",
    )
    .unwrap();
    assert_eq!(
        parsed[0],
        Row {
            key: "a.b_2".to_owned(),
            what: "fine".to_owned(),
            budget: 1200,
            bound: Bound::Max,
            status: Status::Enforced,
            gate: "crates/x/tests/y.rs::z".to_owned(),
            line: 4,
        }
    );
    assert_eq!(
        (parsed[1].bound, parsed[1].status, parsed[1].line),
        (Bound::Exact, Status::Blocked, 5)
    );
}

#[test]
fn a_malformed_row_is_an_error_not_a_skip() {
    let table = "| Objective | Case | Budget | Bound | Status | Gate |\n\
                 |---|---|---|---|---|---|\n\
                 | `a.b` | fine | 1 | exact | enforced | `X` |\n\
                 | `a.c` | no status | 1 | exact | aspirational | `X` |\n\
                 | `a.b` | twice | 1 | max | blocked | something |\n\
                 | a.d | not a key | 1 | max | blocked | something |\n\
                 | `a.e` | short | 1 |\n\
                 | `a.f` | no number | lots | max | blocked | x |\n\
                 | `a.g` | no bound | 1 | most | blocked | x |\n\
                 | `A.h` | upper case | 1 | max | blocked | x |";
    let e = budget::parse(table).unwrap_err();
    for expected in [
        "4: budget must be a number",
        "5: objective a.b appears twice",
        "6: the objective is not",
        "7: expected 6 cells, found 3",
        "8: budget must be a number",
        "9: budget must be a number",
        "10: the objective is not",
    ] {
        assert!(e.contains(expected), "{expected}: {e}");
    }
    assert!(
        e.starts_with("docs/request-budgets.md is malformed:"),
        "{e}"
    );
}

#[test]
fn a_regression_fails_and_names_the_budget() {
    let failure = budget::verdict(&row(Bound::Exact, 10), 11).unwrap();
    assert!(
        failure.starts_with("x.y regressed: measured 11, budget 10 (a case)."),
        "{failure}"
    );
    assert!(budget::verdict(&row(Bound::Max, 10), 11).is_some());
}

#[test]
fn an_exact_saving_fails_until_the_table_records_it() {
    let failure = budget::verdict(&row(Bound::Exact, 10), 7).unwrap();
    assert!(
        failure.starts_with("x.y improved: measured 7, budget 10 (a case)."),
        "{failure}"
    );
    assert!(failure.contains("to 7 so the saving"), "{failure}");
    assert_eq!(budget::verdict(&row(Bound::Exact, 10), 10), None);
    assert_eq!(
        budget::verdict(&row(Bound::Max, 10), 7),
        None,
        "a ceiling is met from below"
    );
    assert_eq!(budget::verdict(&row(Bound::Max, 10), 10), None);
}

#[test]
fn check_refuses_an_unknown_or_blocked_objective() {
    let unknown = std::panic::catch_unwind(|| budget::check("no.such.objective", 0, ""));
    let message = *unknown.unwrap_err().downcast::<String>().unwrap();
    assert_eq!(
        message,
        "no objective no.such.objective in docs/request-budgets.md"
    );
    let blocked = budget::rows()
        .iter()
        .find(|r| r.status == Status::Blocked)
        .unwrap();
    let key = blocked.key.clone();
    assert!(std::panic::catch_unwind(move || budget::check(&key, 0, "")).is_err());
}
