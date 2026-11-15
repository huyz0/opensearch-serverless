//! What the emulator injects into each request: latency, failures and
//! throttling, decided by a seeded generator so a run repeats exactly.
//!
//! The Java shell's test kit injects the same things through its metered
//! test stores (`-Dtests.serverless.meter.latency=flat|s3|none`, seeded); the
//! profile there is GET 20-30 ms, PUT 40-60 ms, LIST 40-70 ms. This module is
//! the mechanism; a profile is a set of [`Latency`] and [`Faults`] values.

use std::time::Duration;

use crate::error::BlobError;

/// How long each request of one kind takes before the emulator answers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    /// Every request waits at least this long.
    pub base: Duration,
    /// Plus a seeded uniform amount below this.
    pub jitter: Duration,
}

impl Latency {
    /// A fixed delay with no jitter.
    pub const fn fixed(base: Duration) -> Self {
        Self {
            base,
            jitter: Duration::ZERO,
        }
    }
}

/// Random faults for one request kind, in parts per million of requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faults {
    /// Requests answered [`BlobError::Unavailable`].
    pub fail_ppm: u32,
    /// Requests answered [`BlobError::Throttled`].
    pub throttle_ppm: u32,
}

const PPM: u64 = 1_000_000;

/// What the emulator does with one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    /// How long to wait before answering.
    pub(crate) delay: Duration,
    /// The fault to answer with instead of doing the work.
    pub(crate) fault: Option<BlobError>,
    /// Do the work, then answer [`BlobError::Unavailable`] as if the
    /// response were lost on the way back.
    pub(crate) lose_response: bool,
}

/// Per-kind injection state: its own generator, the random rates, and
/// scripted faults that hit the next requests of the kind whatever the
/// rates say. Each kind draws from its own generator, so the n-th request
/// of a kind gets the same decision however requests of other kinds
/// interleave with it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OpControl {
    pub(crate) rng: SplitMix64,
    pub(crate) latency: Latency,
    pub(crate) faults: Faults,
    pub(crate) fail_next: u32,
    pub(crate) throttle_next: u32,
    pub(crate) lose_next: u32,
}

impl OpControl {
    /// Decides one request: its delay, and its fault or lost response, if
    /// any. Draws from the generator the same number of times whatever the
    /// outcome.
    pub(crate) fn decide(&mut self) -> Decision {
        let roll = self.rng.next() % PPM;
        let jitter_roll = self.rng.next();
        let delay = self
            .latency
            .base
            .saturating_add(jitter(self.latency.jitter, jitter_roll));
        let fault = if self.fail_next > 0 {
            self.fail_next = self.fail_next.saturating_sub(1);
            Some(BlobError::Unavailable)
        } else if self.throttle_next > 0 {
            self.throttle_next = self.throttle_next.saturating_sub(1);
            Some(BlobError::Throttled)
        } else {
            classify(roll, self.faults)
        };
        let lose_response = fault.is_none() && self.lose_next > 0;
        if lose_response {
            self.lose_next = self.lose_next.saturating_sub(1);
        }
        Decision {
            delay,
            fault,
            lose_response,
        }
    }
}

/// The random fault a roll in `0..PPM` falls on: the first `fail_ppm`
/// values fail, the next `throttle_ppm` throttle, the rest pass.
fn classify(roll: u64, faults: Faults) -> Option<BlobError> {
    let fail = u64::from(faults.fail_ppm);
    if roll < fail {
        Some(BlobError::Unavailable)
    } else if roll < fail.saturating_add(u64::from(faults.throttle_ppm)) {
        Some(BlobError::Throttled)
    } else {
        None
    }
}

/// A uniform duration below `max`, from one draw.
fn jitter(max: Duration, roll: u64) -> Duration {
    let nanos = u64::try_from(max.as_nanos()).unwrap_or(u64::MAX);
    match roll.checked_rem(nanos) {
        Some(n) => Duration::from_nanos(n),
        None => Duration::ZERO,
    }
}

/// SplitMix64: a tiny, fast, seedable generator, good enough to place
/// faults and not meant for anything else.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next(), b.next());
        }
        assert_ne!(SplitMix64::new(1).next(), SplitMix64::new(2).next());
    }

    #[test]
    fn splitmix64_matches_the_reference_sequence() {
        let mut g = SplitMix64::new(0);
        assert_eq!(g.next(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(g.next(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(g.next(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn rolls_split_into_failures_then_throttles_then_passes() {
        let f = Faults {
            fail_ppm: 2,
            throttle_ppm: 3,
        };
        assert_eq!(classify(0, f), Some(BlobError::Unavailable));
        assert_eq!(classify(1, f), Some(BlobError::Unavailable));
        assert_eq!(classify(2, f), Some(BlobError::Throttled));
        assert_eq!(classify(4, f), Some(BlobError::Throttled));
        assert_eq!(classify(5, f), None);
        assert_eq!(classify(0, Faults::default()), None);
    }

    #[test]
    fn jitter_is_the_roll_modulo_its_bound() {
        assert_eq!(jitter(Duration::from_nanos(10), 7), Duration::from_nanos(7));
        assert_eq!(
            jitter(Duration::from_nanos(10), 27),
            Duration::from_nanos(7)
        );
    }

    #[test]
    fn scripted_faults_come_before_random_ones() {
        let mut c = OpControl {
            fail_next: 1,
            throttle_next: 1,
            lose_next: 1,
            ..OpControl::default()
        };
        assert_eq!(c.decide().fault, Some(BlobError::Unavailable));
        assert_eq!(c.decide().fault, Some(BlobError::Throttled));
        let lost = c.decide();
        assert_eq!((lost.fault, lost.lose_response), (None, true));
        assert_eq!((c.decide().fault, c.decide().lose_response), (None, false));
    }

    #[test]
    fn full_rates_always_fault() {
        let mut fail = OpControl {
            faults: Faults {
                fail_ppm: 1_000_000,
                throttle_ppm: 0,
            },
            ..OpControl::default()
        };
        let mut throttle = OpControl {
            faults: Faults {
                fail_ppm: 0,
                throttle_ppm: 1_000_000,
            },
            ..OpControl::default()
        };
        for _ in 0..50 {
            assert_eq!(fail.decide().fault, Some(BlobError::Unavailable));
            assert_eq!(throttle.decide().fault, Some(BlobError::Throttled));
        }
    }

    #[test]
    fn delay_is_base_plus_jitter_below_its_bound() {
        let mut c = OpControl {
            rng: SplitMix64::new(3),
            latency: Latency {
                base: Duration::from_millis(5),
                jitter: Duration::from_millis(2),
            },
            ..OpControl::default()
        };
        for _ in 0..100 {
            let d = c.decide().delay;
            assert!(
                d >= Duration::from_millis(5) && d < Duration::from_millis(7),
                "{d:?}"
            );
        }
        assert_eq!(
            Latency::fixed(Duration::from_millis(1)).jitter,
            Duration::ZERO
        );
        assert_eq!(jitter(Duration::ZERO, 99), Duration::ZERO);
    }
}
