//! Errors: a pool that cannot take a reservation, and a budget that does not fit.

use std::fmt;

use crate::Pool;

/// A pool could not grant a reservation or a buffer.
///
/// The caller refuses the request with [`Exhausted::HTTP_STATUS`] or holds it
/// back until memory is released; it never allocates anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exhausted {
    /// The pool that refused.
    pub pool: Pool,
    /// Bytes asked for.
    pub requested: usize,
    /// Bytes already reserved from the pool when it refused.
    pub in_use: usize,
    /// The pool's limit in bytes.
    pub limit: usize,
}

impl Exhausted {
    /// The HTTP status a refused request answers with: 429 Too Many Requests.
    pub const HTTP_STATUS: u16 = 429;

    /// Whether the request can never fit, even with the pool empty.
    pub const fn exceeds_limit(&self) -> bool {
        self.requested > self.limit
    }
}

impl fmt::Display for Exhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "memory pool [{}] exhausted: requested {} bytes with {} of {} in use",
            self.pool, self.requested, self.in_use, self.limit
        )
    }
}

impl std::error::Error for Exhausted {}

/// A budget configuration that cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The pools together are larger than the node total.
    PoolsExceedTotal {
        /// Sum of every pool's bytes.
        pools: usize,
        /// The node total.
        total: usize,
    },
    /// The pools' sizes overflow `usize` when added.
    Overflow,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::PoolsExceedTotal { pools, total } => {
                write!(
                    f,
                    "memory pools total {pools} bytes, over the node budget of {total}"
                )
            }
            ConfigError::Overflow => f.write_str("memory pool sizes overflow when added"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_message_names_pool_and_numbers() {
        let e = Exhausted {
            pool: Pool::Transport,
            requested: 10,
            in_use: 95,
            limit: 100,
        };
        assert_eq!(
            e.to_string(),
            "memory pool [transport] exhausted: requested 10 bytes with 95 of 100 in use"
        );
        assert!(!e.exceeds_limit());
        assert_eq!(Exhausted::HTTP_STATUS, 429);
    }

    #[test]
    fn exceeds_limit_only_past_the_limit() {
        let at = Exhausted {
            pool: Pool::Transport,
            requested: 100,
            in_use: 1,
            limit: 100,
        };
        let over = Exhausted {
            requested: 101,
            ..at
        };
        assert!(!at.exceeds_limit());
        assert!(over.exceeds_limit());
    }

    #[test]
    fn config_error_messages() {
        let e = ConfigError::PoolsExceedTotal {
            pools: 11,
            total: 10,
        };
        assert_eq!(
            e.to_string(),
            "memory pools total 11 bytes, over the node budget of 10"
        );
        assert_eq!(
            ConfigError::Overflow.to_string(),
            "memory pool sizes overflow when added"
        );
    }
}
