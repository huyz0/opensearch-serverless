//! The writer's swap deadline is read on the boot-time clock, which keeps
//! counting while the machine is suspended or its VM is frozen.

use std::time::Duration;

use oss_writer::boot_clock::BootInstant;

/// An instant's age is at least the time slept, and a later instant is
/// never earlier than an earlier one.
#[test]
fn an_instants_age_counts_the_time_slept() {
    let first = BootInstant::now();
    std::thread::sleep(Duration::from_millis(30));
    let second = BootInstant::now();
    assert!(first.elapsed() >= Duration::from_millis(30));
    assert!(second >= first);
    assert!(second.elapsed() < first.elapsed());
}

/// Going back a duration gives an older instant, or nothing when the clock
/// has not run that long.
#[test]
fn going_back_gives_an_older_instant_or_nothing() {
    let now = BootInstant::now();
    let older = now
        .checked_sub(Duration::from_millis(5))
        .expect("it has run that long");
    assert!(older < now);
    assert!(older.elapsed() >= Duration::from_millis(5));
    assert!(now.checked_sub(Duration::from_secs(u64::MAX / 4)).is_none());
}

/// On Linux the reading is `CLOCK_BOOTTIME`, not `CLOCK_MONOTONIC`: read
/// through the same call, the two agree to within a second.
#[cfg(target_os = "linux")]
#[test]
fn on_linux_the_reading_is_the_boottime_clock() {
    use rustix::time::{ClockId, clock_gettime};
    let raw = |id| {
        let ts = clock_gettime(id);
        Duration::new(
            ts.tv_sec.unsigned_abs(),
            u32::try_from(ts.tv_nsec).unwrap_or(0),
        )
    };
    let before = raw(ClockId::Boottime);
    let ours = BootInstant::now();
    let after = raw(ClockId::Boottime);
    // An instant older than `before` by `d` has an age of at least `d`.
    let probe = ours.checked_sub(Duration::from_secs(0)).expect("zero back");
    assert!(probe.elapsed() <= after.saturating_sub(before) + Duration::from_secs(1));
    assert!(ours.elapsed() <= after.saturating_sub(before) + Duration::from_secs(1));
}
