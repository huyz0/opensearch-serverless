//! The typed failure of a call and the HTTP status it answers.

use crate::status::Status;

/// The most bytes of a module's detail a failure keeps.
pub const MAX_ERROR_DETAIL_BYTES: usize = 256;

/// What kind of failure a call was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The output did not fit its region.
    OutputTooLarge,
    /// The module found the input malformed.
    InvalidArgument,
    /// The module does not know the point or version it declared.
    Unsupported,
    /// A limit stopped the call (the module's or the host's).
    Limit,
    /// The call was cancelled at its deadline.
    Cancelled,
    /// The module reported its own failure.
    ModuleFailed,
    /// A call arrived before init.
    NotInitialized,
    /// The module trapped (an instruction trap, fuel, memory).
    Trap,
    /// The host had no capacity for the call.
    Busy,
    /// The module broke the contract (an undefined status, a bad output).
    Malformed,
}

/// A failed call: its kind and a short, printable detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    kind: FailureKind,
    detail: String,
}

/// A module's text made safe to log: lossy UTF-8, control characters
/// replaced, cut to [`MAX_ERROR_DETAIL_BYTES`] on a character boundary.
fn clean(raw: &[u8]) -> String {
    // ALLOC: one failure, at most MAX_ERROR_DETAIL_BYTES kept of the module's text.
    let mut out = String::new();
    for c in String::from_utf8_lossy(raw).chars() {
        let c = if c.is_control() { '?' } else { c };
        if out.len().saturating_add(c.len_utf8()) > MAX_ERROR_DETAIL_BYTES {
            break;
        }
        out.push(c);
    }
    out
}

impl CallError {
    /// The failure a module's non-zero `status` stands for, with the detail
    /// it wrote (only a module failure carries one).
    pub fn from_status(status: Status, detail: &[u8]) -> Self {
        let (kind, detail) = match status {
            Status::Ok => (FailureKind::Malformed, &[][..]),
            Status::BufferTooSmall => (FailureKind::OutputTooLarge, &[][..]),
            Status::InvalidArgument => (FailureKind::InvalidArgument, detail),
            Status::Unsupported => (FailureKind::Unsupported, &[][..]),
            Status::Limit => (FailureKind::Limit, detail),
            Status::Cancelled => (FailureKind::Cancelled, &[][..]),
            Status::Module => (FailureKind::ModuleFailed, detail),
            Status::NotInitialized => (FailureKind::NotInitialized, &[][..]),
        };
        Self {
            kind,
            detail: clean(detail),
        }
    }

    /// A trap, with the runtime's description.
    pub fn trap(description: &str) -> Self {
        Self {
            kind: FailureKind::Trap,
            detail: clean(description.as_bytes()),
        }
    }

    /// The host had no capacity.
    pub fn busy() -> Self {
        Self {
            kind: FailureKind::Busy,
            // ALLOC: one failure, a short fixed message.
            detail: "no capacity for the call".to_owned(),
        }
    }

    /// The module returned a status the contract does not define.
    pub fn malformed(code: i32) -> Self {
        Self {
            kind: FailureKind::Malformed,
            // ALLOC: one failure, a short message.
            detail: format!("undefined status {code}"),
        }
    }

    /// What kind of failure this was.
    pub fn kind(&self) -> FailureKind {
        self.kind
    }

    /// The module's or runtime's description, printable and bounded.
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// The HTTP status this failure answers: 429 for capacity only, 502 for
    /// a module that broke the contract, 400 for a failure of the input
    /// that a retry would repeat.
    pub fn http_status(&self) -> u16 {
        match self.kind {
            FailureKind::Busy => 429,
            FailureKind::Malformed | FailureKind::NotInitialized | FailureKind::Unsupported => 502,
            _ => 400,
        }
    }
}
