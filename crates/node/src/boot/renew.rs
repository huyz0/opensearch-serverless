//! The thread that keeps the node's lease alive and its view of the fleet
//! fresh.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// When the lease thread renews the node's lease (a PUT) and when it reads
/// the fleet's leases again (GETs): each on its own period, the thread
/// ticking as often as the shorter.
pub(super) struct Cadence {
    renew_every: Duration,
    refresh_every: Duration,
    renewed: Instant,
    refreshed: Instant,
}

impl Cadence {
    /// A cadence starting at `now`.
    pub(super) fn new(renew_every: Duration, refresh_every: Duration, now: Instant) -> Self {
        Self {
            renew_every,
            refresh_every,
            renewed: now,
            refreshed: now,
        }
    }

    /// How often the thread ticks.
    pub(super) fn tick_every(&self) -> Duration {
        self.renew_every.min(self.refresh_every)
    }

    /// Whether the lease is due for renewal at `now`; true once for each
    /// period, whether or not the renewal then succeeds (the next period
    /// tries again, and a lease only has to land once in a lifetime).
    pub(super) fn renew_due(&mut self, now: Instant) -> bool {
        let due = now.duration_since(self.renewed) >= self.renew_every;
        if due {
            self.renewed = now;
        }
        due
    }

    /// Whether the fleet is due to be read again at `now`.
    pub(super) fn refresh_due(&self, now: Instant) -> bool {
        now.duration_since(self.refreshed) >= self.refresh_every
    }

    /// The fleet was read at `now`; a read that failed is not noted, so the
    /// next tick tries again.
    pub(super) fn refreshed(&mut self, now: Instant) {
        self.refreshed = now;
    }
}

/// Runs a tick every interval until dropped.
pub struct Renewal {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Renewal {
    /// Starts the thread: `tick` every `every`.
    pub fn spawn(every: Duration, tick: impl FnMut() + Send + 'static) -> std::io::Result<Self> {
        Self::spawn_named("lease-renewal", every, tick)
    }

    /// Starts a thread called `name`: `tick` every `every`.
    pub fn spawn_named(
        name: &str,
        every: Duration,
        mut tick: impl FnMut() + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let (flag, wake) = &*signal;
                loop {
                    let stopped = flag.lock().unwrap_or_else(PoisonError::into_inner);
                    let (stopped, _) = wake
                        .wait_timeout_while(stopped, every, |stopped| !*stopped)
                        .unwrap_or_else(PoisonError::into_inner);
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    tick();
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Renewal {
    fn drop(&mut self) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            // A tick that panicked already ended the thread.
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn it_ticks_until_dropped_and_then_stops() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&ticks);
        let renewal = Renewal::spawn(Duration::from_millis(5), move || {
            seen.fetch_add(1, Ordering::SeqCst);
        })
        .expect("spawns");
        let start = Instant::now();
        while ticks.load(Ordering::SeqCst) < 3 {
            assert!(start.elapsed() < Duration::from_secs(10), "no ticks");
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(renewal);
        let after = ticks.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(ticks.load(Ordering::SeqCst), after);
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The first tick's reads and renewals over `ticks` ticks, as the thread
    /// would make them (every read succeeding).
    fn run(renew: u64, refresh: u64, ticks: u64) -> (u64, u64) {
        let start = Instant::now();
        let mut cadence = Cadence::new(ms(renew), ms(refresh), start);
        let every = cadence.tick_every();
        let (mut renewals, mut reads) = (0, 0);
        for n in 1..=ticks {
            let now = start
                .checked_add(every.saturating_mul(u32::try_from(n).expect("small")))
                .expect("in range");
            if cadence.renew_due(now) {
                renewals = u64::saturating_add(renewals, 1);
            }
            if cadence.refresh_due(now) {
                reads = u64::saturating_add(reads, 1);
                cadence.refreshed(now);
            }
        }
        (renewals, reads)
    }

    #[test]
    fn with_equal_periods_each_tick_renews_and_reads() {
        assert_eq!(run(1000, 1000, 10), (10, 10));
    }

    #[test]
    fn a_faster_read_ticks_at_its_period_and_renews_on_its_own() {
        // Reading every 200 ms for 10 s: 50 reads, and the 1 s renewal 10.
        assert_eq!(
            Cadence::new(ms(1000), ms(200), Instant::now()).tick_every(),
            ms(200)
        );
        assert_eq!(run(1000, 200, 50), (10, 50));
    }

    #[test]
    fn a_slower_read_ticks_at_the_renewal_and_reads_on_its_own() {
        // Renewing every 1 s for 10 s: 10 renewals, and the 2.5 s read 3 (at
        // ticks 3, 6 and 9: a read lands at the first tick past its period).
        assert_eq!(
            Cadence::new(ms(1000), ms(2500), Instant::now()).tick_every(),
            ms(1000)
        );
        assert_eq!(run(1000, 2500, 10), (10, 3));
    }

    #[test]
    fn a_read_that_failed_is_due_again_at_the_next_tick() {
        let start = Instant::now();
        let mut cadence = Cadence::new(ms(1000), ms(1000), start);
        let tick = start + ms(1000);
        assert!(cadence.refresh_due(tick));
        // Not noted: still due a tick later.
        assert!(cadence.refresh_due(tick + ms(1000)));
        cadence.refreshed(tick);
        assert!(!cadence.refresh_due(tick + ms(500)));
    }

    #[test]
    fn a_renewal_is_due_once_a_period_even_when_it_failed() {
        let start = Instant::now();
        let mut cadence = Cadence::new(ms(1000), ms(1000), start);
        assert!(!cadence.renew_due(start + ms(999)));
        assert!(cadence.renew_due(start + ms(1000)));
        assert!(
            !cadence.renew_due(start + ms(1500)),
            "not again before the period"
        );
        assert!(cadence.renew_due(start + ms(2000)));
    }
}
