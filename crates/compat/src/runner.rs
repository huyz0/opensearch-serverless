//! Replays fixtures against a target and reports, per step, whether the
//! normalised responses match.

use std::fmt;

use crate::compare::{Differences, compare};
use crate::fixture::{Fixture, Step};
use crate::normalise::normalise;
use crate::target::{Target, TargetError};

/// How one step went.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// The normalised responses are equal.
    Passed,
    /// They differ, or a rule is stale (the fixture needs fixing).
    Failed {
        /// The differences after normalisation.
        differences: Differences,
        /// Rules that matched nothing in the recording.
        stale_rules: Vec<String>,
    },
    /// The fixture has no recorded response for this step.
    NotRecorded,
    /// The target produced no response.
    TargetFailed(TargetError),
}

/// One step's result.
#[derive(Debug, Clone, PartialEq)]
pub struct StepReport {
    /// The step's name.
    pub name: String,
    /// What happened.
    pub outcome: StepOutcome,
}

/// One fixture's result.
#[derive(Debug, Clone, PartialEq)]
pub struct FixtureReport {
    /// The fixture's name.
    pub name: String,
    /// Every step's result, in order.
    pub steps: Vec<StepReport>,
}

impl FixtureReport {
    /// Every step passed.
    pub fn passed(&self) -> bool {
        self.steps.iter().all(|s| s.outcome == StepOutcome::Passed)
    }
}

impl fmt::Display for FixtureReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = if self.passed() { "ok" } else { "FAILED" };
        writeln!(f, "{}: {verdict}", self.name)?;
        for step in &self.steps {
            match &step.outcome {
                StepOutcome::Passed => writeln!(f, "  {}: ok", step.name)?,
                StepOutcome::NotRecorded => writeln!(f, "  {}: not recorded", step.name)?,
                StepOutcome::TargetFailed(e) => writeln!(f, "  {}: target error: {e}", step.name)?,
                StepOutcome::Failed {
                    differences,
                    stale_rules,
                } => {
                    writeln!(f, "  {}: differs", step.name)?;
                    for rule in stale_rules {
                        writeln!(f, "    stale rule {rule}: matches nothing in the recording")?;
                    }
                    for line in differences.to_string().lines() {
                        writeln!(f, "    {line}")?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A whole run's result.
#[derive(Debug, Clone, PartialEq)]
pub struct SuiteReport {
    /// Every fixture's result, in the order run.
    pub fixtures: Vec<FixtureReport>,
}

impl SuiteReport {
    /// Every fixture passed.
    pub fn passed(&self) -> bool {
        self.fixtures.iter().all(FixtureReport::passed)
    }
}

impl fmt::Display for SuiteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for fixture in &self.fixtures {
            write!(f, "{fixture}")?;
        }
        let failed = self.fixtures.iter().filter(|r| !r.passed()).count();
        writeln!(f, "{} fixtures, {failed} failed", self.fixtures.len())
    }
}

/// Runs one fixture: its cleanup requests (responses ignored), then each
/// step. A failing step does not stop the ones after it.
pub fn run_fixture(target: &mut dyn Target, fixture: &Fixture) -> FixtureReport {
    for request in &fixture.cleanup {
        // Cleanup only makes the target fresh; whether the index existed is
        // not part of the comparison.
        let _ = target.send(request);
    }
    FixtureReport {
        name: fixture.name.clone(),
        steps: fixture
            .steps
            .iter()
            .map(|step| StepReport {
                name: step.name.clone(),
                outcome: run_step(target, step),
            })
            .collect(),
    }
}

/// Runs every fixture in order.
pub fn run_suite(target: &mut dyn Target, fixtures: &[Fixture]) -> SuiteReport {
    SuiteReport {
        fixtures: fixtures.iter().map(|f| run_fixture(target, f)).collect(),
    }
}

fn run_step(target: &mut dyn Target, step: &Step) -> StepOutcome {
    let actual = match target.send(&step.request) {
        Ok(actual) => actual,
        Err(e) => return StepOutcome::TargetFailed(e),
    };
    let Some(recorded) = &step.response else {
        return StepOutcome::NotRecorded;
    };
    let mut expected_doc = recorded.to_document(None);
    let mut actual_doc = actual.to_document(Some(&recorded.headers));
    let stale_rules = normalise(&step.normalise, &mut expected_doc, &mut actual_doc);
    let differences = compare(&expected_doc, &actual_doc);
    if differences.is_empty() && stale_rules.is_empty() {
        StepOutcome::Passed
    } else {
        StepOutcome::Failed {
            differences,
            stale_rules,
        }
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
