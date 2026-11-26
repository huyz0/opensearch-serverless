//! The thread that drains the reclaim queue.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::DeleteService;
use crate::restore_background::BackgroundRestores;

/// Runs [`DeleteService::drain`] every interval until dropped.
pub struct DrainTimer {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    restores: Option<Arc<BackgroundRestores>>,
}

impl DrainTimer {
    /// Starts the thread: one drain every `every`. A failed drain is tried
    /// again next tick; its intents are kept.
    pub fn spawn(service: Arc<DeleteService>, every: Duration) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("reclaim-drain".to_owned())
            .spawn(move || {
                let (flag, wake) = &*signal;
                loop {
                    let stopped = flag.lock().unwrap_or_else(PoisonError::into_inner);
                    // Waits only while not stopped: a stop sent before the wait began
                    // is not lost.
                    let (stopped, _) = wake
                        .wait_timeout_while(stopped, every, |stopped| !*stopped)
                        .unwrap_or_else(PoisonError::into_inner);
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    let _ = service.drain();
                }
            })
            .ok();
        Self {
            stop,
            thread,
            restores: None,
        }
    }

    /// Drains `restores` when this timer is dropped: the node's shutdown
    /// stops the background copies before the reclaim thread ends.
    #[must_use]
    pub fn with_restores(mut self, restores: Option<Arc<BackgroundRestores>>) -> Self {
        self.restores = restores;
        self
    }
}

impl Drop for DrainTimer {
    fn drop(&mut self) {
        // Copies stop first: what a cancelled copy deletes is not left for
        // a reclaim that no longer runs.
        if let Some(restores) = &self.restores {
            let _ = restores.drain();
        }
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
