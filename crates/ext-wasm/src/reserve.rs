//! Memory is reserved before it is used; the pool that grants it is the
//! caller's.

/// Something held until dropped: the reservation's release.
pub type Reservation = Box<dyn Send + Sync>;

/// Grants reservations from a bounded pool.
pub trait Reserver: Send + Sync {
    /// Reserves `bytes`, or `None` when the pool cannot hold them.
    fn reserve(&self, bytes: u64) -> Option<Reservation>;
}

/// A reserver that always grants; for tests and for hosts that reserve
/// elsewhere.
pub struct Unbounded;

impl Reserver for Unbounded {
    fn reserve(&self, _bytes: u64) -> Option<Reservation> {
        // ALLOC: one zero-sized guard.
        Some(Box::new(()))
    }
}
