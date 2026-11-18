//! The share of the heap the indexing buffers of a node's shards may use.

/// Core's default for `indices.memory.index_buffer_size`: 10% of the heap.
pub const DEFAULT_PERCENT_BASIS_POINTS: u32 = 1_000;

/// Core's default for `indices.memory.min_index_buffer_size`: 48 MiB.
pub const DEFAULT_MIN_BYTES: u64 = 48 << 20;

const BASIS_POINTS: u128 = 10_000;

/// How `indices.memory.index_buffer_size` was set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferSize {
    /// A share of the heap, in hundredths of a percent (1,000 is 10%).
    Percent(u32),
    /// A number of bytes.
    Bytes(u64),
}

/// The bytes the indexing buffers of all shards on a node share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexingBudget(u64);

impl IndexingBudget {
    /// The budget `size` gives over a heap of `heap` bytes. A percentage is
    /// raised to `min` and capped at `max` when one is set; bytes are used
    /// as they are (`computeBufferSize`).
    pub fn resolve(size: BufferSize, heap: u64, min: u64, max: Option<u64>) -> Self {
        let bytes = match size {
            BufferSize::Bytes(bytes) => bytes,
            BufferSize::Percent(basis_points) => {
                let share = u128::from(heap)
                    .saturating_mul(u128::from(basis_points))
                    .checked_div(BASIS_POINTS)
                    .unwrap_or(0);
                let share = u64::try_from(share).unwrap_or(u64::MAX).max(min);
                max.map_or(share, |ceiling| share.min(ceiling))
            }
        };
        Self(bytes)
    }

    /// The budget in bytes.
    pub fn bytes(self) -> u64 {
        self.0
    }

    /// The bytes written between checks: a thirtieth of the budget.
    pub fn check_interval_bytes(self) -> u64 {
        self.0.checked_div(30).unwrap_or(0)
    }

    /// Whether `bytes` is past one and a half budgets.
    pub fn past_one_and_a_half(self, bytes: u64) -> bool {
        u128::from(bytes).saturating_mul(2) > u128::from(self.0).saturating_mul(3)
    }
}
