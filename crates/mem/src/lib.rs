//! The node memory budget and pooled buffers.
//!
//! Every node has a fixed memory budget split into named pools. A component
//! reserves from its pool before it allocates and releases on drop; when a
//! pool is exhausted the caller answers 429 or applies backpressure. I/O uses
//! fixed-size buffers from a per-node pool. See
//! `docs/adr/0004-bounded-memory-and-streaming.md`.
//!
//! ```
//! use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
//!
//! let config = BudgetConfig::new(1 << 20)
//!     .with_pool(Pool::RequestBodies, 512 << 10)?
//!     .with_pool(Pool::Transport, 256 << 10)?;
//! let budget = MemoryBudget::new(&config);
//!
//! let body = budget.try_reserve(Pool::RequestBodies, 4096).expect("fits");
//! assert_eq!(budget.in_use(Pool::RequestBodies), 4096);
//! drop(body);
//! assert_eq!(budget.in_use(Pool::RequestBodies), 0);
//!
//! let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 2).expect("fits");
//! let mut buf = buffers.try_checkout().expect("a buffer is free");
//! buf[0] = 7;
//! # Ok::<(), oss_mem::ConfigError>(())
//! ```

mod budget;
mod buffer;
mod error;
mod pool;
mod reservation;

pub use budget::{BudgetConfig, MemoryBudget};
pub use buffer::{BufferPool, LARGE_BUFFER, PooledBuffer, SMALL_BUFFER};
pub use error::{ConfigError, Exhausted};
pub use pool::Pool;
pub use reservation::Reservation;
