//! The boot-time clock the writer's swap deadline is read on.
//!
//! `CLOCK_BOOTTIME` keeps counting while the machine is suspended or its VM
//! is frozen, where `std::time::Instant` (`CLOCK_MONOTONIC`) stops: a writer
//! paused past the deadline must find it expired, so the age is read here.
//! Where there is no such clock the reading falls back to the monotonic one.

use std::time::Duration;

/// A point on the boot-time clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BootInstant(Duration);

impl BootInstant {
    /// Now.
    pub fn now() -> Self {
        Self(reading())
    }

    /// How long ago this was; zero if the clock reads earlier.
    pub fn elapsed(&self) -> Duration {
        reading().saturating_sub(self.0)
    }

    /// This instant moved back by `by`, or `None` when the clock has not
    /// run that long.
    pub fn checked_sub(self, by: Duration) -> Option<Self> {
        self.0.checked_sub(by).map(Self)
    }
}

#[cfg(target_os = "linux")]
fn reading() -> Duration {
    use rustix::time::{ClockId, clock_gettime};
    let at = clock_gettime(ClockId::Boottime);
    Duration::new(
        u64::try_from(at.tv_sec).unwrap_or(0),
        u32::try_from(at.tv_nsec).unwrap_or(0),
    )
}

#[cfg(not(target_os = "linux"))]
fn reading() -> Duration {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}
