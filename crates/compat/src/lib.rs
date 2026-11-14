//! The OpenSearch compatibility suite.
//!
//! Replays golden fixtures recorded from stock OpenSearch against a
//! [`Target`] and compares normalised responses. See
//! `docs/compatibility.md` for what is in scope and
//! `fixtures/compat/README.md` for the fixture format and how fixtures are
//! recorded.
//!
//! The pieces, in the order a run uses them: [`load_dir`] reads fixture files
//! with a bounded size, [`Fixture`] is the parsed format, [`run_suite`] sends
//! each request to the target, [`normalise`](normalise::normalise) blanks the
//! fields that legitimately vary, and [`compare()`] reports what differs.

pub mod compare;
pub mod fixture;
pub mod load;
pub mod normalise;
pub mod pointer;
pub mod runner;
pub mod target;

pub use compare::{Difference, Differences, compare};
pub use fixture::{Body, Fixture, FixtureError, Method, Request, Response, Rule, Step};
pub use load::{LoadError, MAX_FIXTURE_BYTES, load_dir, load_file};
pub use runner::{FixtureReport, StepOutcome, SuiteReport, run_fixture, run_suite};
pub use target::{Target, TargetError};
