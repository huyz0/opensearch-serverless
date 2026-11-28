//! The alerting runner: which monitors this node runs, found without polling
//! any monitor, and their runs (ADR 0075).

mod schedule;

pub use schedule::{Scheduler, TickReport};
