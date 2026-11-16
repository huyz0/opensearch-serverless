//! docs/specs/request-budgets/SPEC.md AC-2: record mode checks nothing.
//! The test runs its own binary again with the variable set, so no test
//! changes the environment of a running process.

use std::process::Command;

use oss_testkit::budget;

/// Marks the run of this binary that the test started.
const CHILD: &str = "OSS_BUDGET_RECORD_TEST_CHILD";

#[test]
fn record_mode_checks_nothing() {
    if std::env::var_os(CHILD).is_some() {
        // The child: an unknown key fails a check; recorded, it passes. A
        // child never runs a child of its own.
        budget::check("no.such.objective", 99, "recorded, not checked");
        return;
    }
    assert!(!budget::recording());
    let me = std::env::current_exe().unwrap();
    let run = |value: &str| {
        Command::new(&me)
            .args(["--exact", "record_mode_checks_nothing", "--nocapture"])
            .env(budget::RECORD, value)
            .env(CHILD, "1")
            .output()
            .unwrap()
    };
    let recorded = run("1");
    assert!(recorded.status.success(), "{recorded:?}");
    let printed = String::from_utf8_lossy(&recorded.stderr);
    assert!(
        printed.contains("request budget no.such.objective: measured 99\nrecorded, not checked"),
        "{printed}"
    );
    assert!(run("true").status.success());
    // Any other value checks, and the unknown key fails.
    assert!(!run("0").status.success());
}
