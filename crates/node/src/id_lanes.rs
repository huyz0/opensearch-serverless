//! Ids for documents written without one, drawn without a lock held across
//! a request.
//!
//! A node gives time-based ids from its address, a clock and a sequence. A
//! generator shared behind a lock held for a whole request would serialise
//! every write request on the node, including the wait for its log commit.
//! Instead each request takes its own generator on a lane: the node's
//! address with its last two bytes replaced by the lane, and a sequence
//! seeded from it, so requests never draw the same id from different
//! generators until the lane counter wraps.

use std::sync::atomic::{AtomicU32, Ordering};

use oss_writer::bulk::IdGenerator;

/// The lanes a node cycles through.
const LANES: u32 = 1 << 16;

/// The sequence numbers a lane starts apart: its ids stay distinct from the
/// next lane's by the address, this only spreads the sequences.
const SPREAD: u32 = 1 << 8;

/// Hands out one [`IdGenerator`] a write request.
#[derive(Debug)]
pub struct IdLanes {
    address: [u8; 6],
    next: AtomicU32,
}

impl IdLanes {
    /// Lanes over the node's `address`.
    pub fn new(address: [u8; 6]) -> Self {
        Self {
            address,
            next: AtomicU32::new(0),
        }
    }

    /// A generator on the next lane.
    pub fn generator(&self) -> IdGenerator {
        let lane = self.next.fetch_add(1, Ordering::Relaxed) % LANES;
        let [a0, a1, a2, a3, _, _] = self.address;
        let [hi, lo] = u16::try_from(lane).unwrap_or(0).to_be_bytes();
        IdGenerator::new(lane.wrapping_mul(SPREAD), [a0, a1, a2, a3, hi, lo])
    }
}
