//! The HTTP edge's limits.
//!
//! The request-size defaults are OpenSearch's `http.*` defaults
//! (server/src/main/java/org/opensearch/http/HttpTransportSettings.java at
//! huyz0/OpenSearch@49f238ed). The thread, connection and buffer counts are
//! this server's own: OpenSearch runs on Netty's event loops and has none.

use std::time::Duration;

use oss_mem::SMALL_BUFFER;

/// How the server is sized and what it accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    /// Worker threads; each serves one connection at a time.
    pub workers: usize,
    /// Open connections, served and waiting; one more is answered 503 and
    /// closed. At least `workers`.
    pub max_connections: usize,
    /// `http.max_initial_line_length`: the request line's longest length.
    pub max_initial_line: usize,
    /// `http.max_header_size`: the header block's largest size.
    pub max_header_bytes: usize,
    /// The most headers a request may carry.
    pub max_headers: usize,
    /// The most query parameters a request may carry.
    pub max_query_params: usize,
    /// `http.max_content_length`: the largest body.
    pub max_content_length: u64,
    /// The size of each body buffer: the largest chunk a handler receives.
    pub body_buffer_size: usize,
    /// How many requests may be reading a body at once, node-wide.
    pub body_buffers: usize,
    /// How long a request waits for a free body buffer before 429.
    pub body_wait: Duration,
    /// The response write buffer: the largest chunk sent.
    pub write_buffer_size: usize,
    /// How long an open connection may sit between requests.
    pub idle_timeout: Duration,
    /// How long a request's head may take to arrive, and how long a body
    /// read or a response write may stay blocked beyond what
    /// `min_transfer_rate` earns.
    pub request_timeout: Duration,
    /// The slowest transfer, in bytes a second, a peer may hold a worker
    /// with: each byte read or written earns `1 / min_transfer_rate` seconds
    /// of blocked I/O on top of `request_timeout`.
    pub min_transfer_rate: u64,
    /// How often an idle worker checks whether a waiting connection should
    /// take its place.
    pub idle_poll: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            workers: 128,
            max_connections: 1024,
            max_initial_line: 4 << 10,
            max_header_bytes: 16 << 10,
            max_headers: 256,
            max_query_params: 256,
            max_content_length: 100 << 20,
            body_buffer_size: SMALL_BUFFER,
            body_buffers: 128,
            body_wait: Duration::from_secs(30),
            write_buffer_size: 16 << 10,
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
            min_transfer_rate: 64 << 10,
            idle_poll: Duration::from_millis(50),
        }
    }
}

/// A configuration the server cannot run with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A count or size that must be positive is zero.
    Zero(&'static str),
    /// `max_connections` is below `workers`.
    FewerConnectionsThanWorkers,
    /// The write buffer cannot hold a response head.
    WriteBufferTooSmall,
}

/// The smallest write buffer: room for a status line and the headers the
/// edge writes.
pub(crate) const MIN_WRITE_BUFFER: usize = 1024;

impl ServerConfig {
    /// Checks the configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let positive = [
            ("workers", self.workers),
            ("max_initial_line", self.max_initial_line),
            ("max_header_bytes", self.max_header_bytes),
            ("max_headers", self.max_headers),
            ("body_buffer_size", self.body_buffer_size),
            ("body_buffers", self.body_buffers),
        ];
        if let Some((name, _)) = positive.iter().find(|(_, v)| *v == 0) {
            return Err(ConfigError::Zero(name));
        }
        if self.idle_poll.is_zero() || self.request_timeout.is_zero() {
            return Err(ConfigError::Zero("timeouts"));
        }
        if self.min_transfer_rate == 0 {
            return Err(ConfigError::Zero("min_transfer_rate"));
        }
        if self.max_connections < self.workers {
            return Err(ConfigError::FewerConnectionsThanWorkers);
        }
        if self.write_buffer_size < MIN_WRITE_BUFFER {
            return Err(ConfigError::WriteBufferTooSmall);
        }
        Ok(())
    }

    /// The head buffer: the request line, the headers, their line endings
    /// and the blank line.
    pub(crate) fn head_buffer_size(&self) -> usize {
        self.max_initial_line
            .saturating_add(self.max_header_bytes)
            .saturating_add(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_valid_and_matches_opensearch_sizes() {
        let c = ServerConfig::default();
        assert_eq!(c.validate(), Ok(()));
        assert_eq!(
            (c.max_initial_line, c.max_header_bytes, c.max_content_length),
            (4096, 16384, 104_857_600)
        );
        assert_eq!(c.head_buffer_size(), 4096 + 16384 + 4);
    }

    #[test]
    fn invalid_configurations_are_refused() {
        let c = ServerConfig {
            workers: 0,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::Zero("workers")));
        let c = ServerConfig {
            body_buffers: 0,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::Zero("body_buffers")));
        let c = ServerConfig {
            idle_poll: Duration::ZERO,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::Zero("timeouts")));
        let c = ServerConfig {
            min_transfer_rate: 0,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::Zero("min_transfer_rate")));
        let c = ServerConfig {
            workers: 4,
            max_connections: 3,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::FewerConnectionsThanWorkers));
        let c = ServerConfig {
            workers: 4,
            max_connections: 4,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Ok(()));
        let c = ServerConfig {
            write_buffer_size: MIN_WRITE_BUFFER - 1,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::WriteBufferTooSmall));
    }
}
