//! The thread that keeps the node's lease alive and its view of the fleet
//! fresh.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// Runs a tick every interval until dropped.
pub struct Renewal {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Renewal {
    /// Starts the thread: `tick` every `every`.
    pub fn spawn(every: Duration, mut tick: impl FnMut() + Send + 'static) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("lease-renewal".to_owned())
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
            })
            .ok();
        Self { stop, thread }
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
    use std::time::Instant;

    #[test]
    fn it_ticks_until_dropped_and_then_stops() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&ticks);
        let renewal = Renewal::spawn(Duration::from_millis(5), move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
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
}
