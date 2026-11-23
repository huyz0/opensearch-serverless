//! The thread that runs the writer role's publish pass.
//!
//! The shell's `BackgroundReconciler` runs its publish pass on a schedule;
//! this is that schedule for [`WriterRole::publish_due`].

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::WriterRole;

/// Runs `publish_due` every interval until dropped.
pub struct PublishTimer {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl PublishTimer {
    /// Starts the thread: one pass of `role` every `every`.
    pub fn spawn(role: Arc<WriterRole>, every: Duration) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("publish-timer".to_owned())
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
                    role.publish_due();
                    role.hibernate_due(super::now_millis());
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for PublishTimer {
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
