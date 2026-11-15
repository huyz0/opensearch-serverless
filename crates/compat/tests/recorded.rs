//! Checks the recorded fixtures under `fixtures/compat/` with nothing but
//! this repository: they load, every step is recorded, every rule matches
//! its recording, and the runner passes them against a target that answers
//! with the recordings.
//!
//! With `COMPAT_REPLAY_FROM=<dir>` set to a second recording of the same
//! fixtures (`record.py` run on a copy), the target answers with that
//! recording instead, which shows the normalisation rules cover everything
//! that varies between two runs of stock OpenSearch.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use oss_compat::{
    Fixture, Request, Response, StepOutcome, Target, TargetError, load_dir, run_suite,
};
use serde_json::Value;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat")
}

/// Answers each fixture's cleanup with a 200 and each step with a given
/// response, in order.
struct ReplayTarget {
    answers: VecDeque<Response>,
}

impl ReplayTarget {
    fn new(fixtures: &[Fixture]) -> Self {
        let ok = Response {
            status: 200,
            headers: Default::default(),
            body: None,
        };
        let mut answers = VecDeque::new();
        for f in fixtures {
            answers.extend(f.cleanup.iter().map(|_| ok.clone()));
            answers.extend(
                f.steps
                    .iter()
                    .map(|s| s.response.clone().expect("recorded")),
            );
        }
        Self { answers }
    }
}

impl Target for ReplayTarget {
    fn send(&mut self, _: &Request) -> Result<Response, TargetError> {
        self.answers.pop_front().ok_or_else(|| TargetError {
            message: "replay ran out of responses".into(),
        })
    }
}

/// Changes every value a rule covers, so a pass shows the rules took effect
/// rather than that the documents were identical.
fn perturbed(fixtures: &[Fixture]) -> Vec<Fixture> {
    let mut out = fixtures.to_vec();
    for f in &mut out {
        for step in &mut f.steps {
            let response = step.response.as_mut().expect("recorded");
            let mut doc = response.to_document(None);
            for rule in &step.normalise {
                rule.pointer
                    .replace_all(&mut doc, &Value::from("perturbed"));
            }
            response.body = match doc["body"].take() {
                Value::Null => None,
                body => Some(body),
            };
        }
    }
    out
}

fn loaded() -> Vec<(PathBuf, Fixture)> {
    let fixtures = load_dir(&fixtures_root()).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !fixtures.is_empty(),
        "no fixtures under {}",
        fixtures_root().display()
    );
    fixtures
}

#[test]
fn recorded_fixtures_are_well_formed() {
    for (path, fixture) in loaded() {
        let from = fixture
            .recorded_from
            .as_ref()
            .unwrap_or_else(|| panic!("{}: not recorded", path.display()));
        assert!(
            from.version.starts_with("3.10.0"),
            "{}: recorded from {}",
            path.display(),
            from.version
        );
        for step in &fixture.steps {
            let response = step
                .response
                .as_ref()
                .unwrap_or_else(|| panic!("{}: step {:?} not recorded", path.display(), step.name));
            let mut doc = response.to_document(None);
            for rule in &step.normalise {
                let matched = rule.pointer.replace_all(&mut doc, &Value::Null);
                assert!(
                    matched > 0,
                    "{}: step {:?}: stale rule {}",
                    path.display(),
                    step.name,
                    rule.pointer
                );
            }
        }
    }
}

#[test]
fn recorded_fixtures_replay_cleanly() {
    let fixtures: Vec<Fixture> = loaded().into_iter().map(|(_, f)| f).collect();
    let answers = match std::env::var_os("COMPAT_REPLAY_FROM") {
        None => perturbed(&fixtures),
        Some(dir) => {
            let mut by_name: HashMap<String, Fixture> = load_dir(Path::new(&dir))
                .unwrap_or_else(|e| panic!("{e}"))
                .into_iter()
                .map(|(_, f)| (f.name.clone(), f))
                .collect();
            fixtures
                .iter()
                .map(|f| {
                    by_name
                        .remove(&f.name)
                        .expect("same fixtures in both recordings")
                })
                .collect()
        }
    };
    let report = run_suite(&mut ReplayTarget::new(&answers), &fixtures);
    assert!(report.passed(), "{report}");
    let steps: usize = report.fixtures.iter().map(|f| f.steps.len()).sum();
    assert!(
        report
            .fixtures
            .iter()
            .flat_map(|f| &f.steps)
            .all(|s| s.outcome == StepOutcome::Passed)
    );
    assert_eq!(steps, fixtures.iter().map(|f| f.steps.len()).sum::<usize>());
}
