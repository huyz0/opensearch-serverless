//! The system under test, as the runner sees it.

use std::fmt;

use crate::fixture::{Request, Response};

/// Something that answers REST requests: `oss-node` in process, a server
/// over HTTP, or a fake in the harness's own tests.
pub trait Target {
    /// Sends one request and returns the response. Header names in the
    /// response must be lower-cased. An `Err` means no HTTP response was
    /// obtained (connection refused, timeout); an error status is an `Ok`.
    fn send(&mut self, request: &Request) -> Result<Response, TargetError>;
}

/// The target produced no HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetError {
    /// What went wrong.
    pub message: String,
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TargetError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_error_displays_its_message() {
        let e = TargetError {
            message: "connection refused".into(),
        };
        assert_eq!(e.to_string(), "connection refused");
    }
}
