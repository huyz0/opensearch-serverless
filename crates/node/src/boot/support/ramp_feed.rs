//! What a joining reader's slow start reads at each lease renewal: the
//! searches this node answered and the store GETs they cost, which say how
//! warm its cache is (docs/specs/reader-slow-start).

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::S3Op;
use oss_cluster::placement::{ControllerConfig, RampController, Totals};

use crate::lifecycle::Readiness;

/// The controller and what it reads.
pub(super) struct RampFeed {
    readiness: Arc<Readiness>,
    ledger: Arc<oss_blob::usage::Ledger>,
    controller: Mutex<RampController>,
}

impl RampFeed {
    /// A feed when the slow start is on and the store's usage is counted;
    /// otherwise `None`, and time alone bounds the ramp.
    pub(super) fn new(
        readiness: &Arc<Readiness>,
        ledger: Option<&Arc<oss_blob::usage::Ledger>>,
    ) -> Option<Self> {
        let ledger = ledger.filter(|_| oss_cluster::placement::ramp_config().is_on())?;
        Some(Self {
            readiness: Arc::clone(readiness),
            ledger: Arc::clone(ledger),
            controller: Mutex::new(RampController::new(
                ControllerConfig::default(),
                Totals {
                    searches: 0,
                    gets_milli: 0,
                },
            )),
        })
    }

    fn totals(&self) -> Totals {
        Totals {
            searches: oss_cluster::placement::searches_total(),
            gets_milli: self
                .ledger
                .total_requests(S3Op::Get)
                .saturating_add(self.ledger.total_requests(S3Op::GetRange)),
        }
    }

    /// Before a renewal: reads the window and sets the ramp the lease will say.
    pub(super) fn renewing(&self) {
        let mut controller = self
            .controller
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.readiness.set_ramp(controller.observe(self.totals()));
    }
}
