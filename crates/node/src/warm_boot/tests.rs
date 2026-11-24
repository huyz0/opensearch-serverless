use super::*;
use std::cell::Cell;

/// A clock the pauses move.
struct Clock(Cell<Duration>);

impl Pause for Clock {
    fn pause(&self, duration: Duration) {
        self.0.set(self.0.get().saturating_add(duration));
    }
}

struct Rig {
    readiness: Readiness,
    progress: Progress,
    done: AtomicBool,
    expired: AtomicBool,
    clock: Clock,
}

fn rig() -> Rig {
    Rig {
        readiness: Readiness::warming(),
        progress: Progress::default(),
        done: AtomicBool::new(false),
        expired: AtomicBool::new(false),
        clock: Clock(Cell::new(Duration::ZERO)),
    }
}

fn join() -> Join {
    Join {
        ready_percent: 90,
        max_wait: Duration::from_secs(60),
        poll: Duration::from_secs(1),
    }
}

fn run(rig: &Rig) -> bool {
    finish_when_warm(
        &rig.readiness,
        &rig.progress,
        &rig.done,
        &join(),
        &|| rig.clock.0.get(),
        &rig.clock,
        &rig.expired,
    )
}

// docs/specs/reader-warm-copy AC-7
#[test]
fn the_node_is_let_in_at_the_ready_percent_of_what_was_named() {
    let rig = rig();
    rig.progress.seed(1000, 900);
    assert!(!run(&rig));
    assert!(rig.readiness.ready());
    assert_eq!(rig.clock.0.get(), Duration::ZERO, "no wait");
    assert!(!rig.expired.load(Ordering::Acquire));

    let short = self::rig();
    short.progress.seed(1000, 899);
    assert!(
        run(&short),
        "899 of 1000 is not 90%, so it waits out the time"
    );
    assert_eq!(short.clock.0.get(), Duration::from_secs(60));
}

// docs/specs/reader-warm-copy AC-6, AC-7
#[test]
fn the_wait_runs_out_at_max_wait_and_stops_the_copy() {
    let rig = rig();
    rig.progress.seed(1000, 0);
    assert!(run(&rig));
    assert!(rig.readiness.ready());
    assert!(rig.expired.load(Ordering::Acquire));
    assert_eq!(rig.clock.0.get(), Duration::from_secs(60));
}

// docs/specs/reader-warm-copy AC-6
#[test]
fn nothing_named_waits_until_the_copy_is_done_and_then_lets_the_node_in() {
    let rig = rig();
    rig.done.store(true, Ordering::Release);
    assert!(!run(&rig));
    assert!(rig.readiness.ready());
    assert!(!rig.expired.load(Ordering::Acquire));

    let waiting = self::rig();
    // Nothing named and the copy not done: the time decides.
    assert!(run(&waiting));
    assert_eq!(waiting.clock.0.get(), Duration::from_secs(60));
}
