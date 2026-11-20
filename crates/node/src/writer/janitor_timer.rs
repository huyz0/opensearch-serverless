//! The thread that runs the janitor pass.
//!
//! The shell's `BackgroundReconciler` runs its collector passes on a
//! schedule; this is that schedule for [`JanitorPass::run`].

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use oss_janitor::gc::Config;

use super::JanitorPass;

/// What a pass's sweeps are told, read at the start of each pass so a
/// dynamic setting takes effect without a restart.
pub type SweepConfig = Box<dyn Fn() -> Config + Send + Sync>;

/// Runs [`JanitorPass::run`] every interval until dropped.
pub struct JanitorTimer {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl JanitorTimer {
    /// Starts the thread: one pass every `every`, under the config `sweep`
    /// answers at that moment.
    pub fn spawn(pass: Arc<JanitorPass>, every: Duration, sweep: SweepConfig) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("janitor-timer".to_owned())
            .spawn(move || {
                let (flag, wake) = &*signal;
                loop {
                    let stopped = flag.lock().unwrap_or_else(PoisonError::into_inner);
                    // A wake that is not the drop's (spurious) runs the pass a
                    // little early, which the pass does not mind.
                    let (stopped, _) = wake
                        .wait_timeout(stopped, every)
                        .unwrap_or_else(PoisonError::into_inner);
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    pass.run(sweep());
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for JanitorTimer {
    fn drop(&mut self) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            // A pass that panicked already ended the thread.
            let _ = thread.join();
        }
    }
}
