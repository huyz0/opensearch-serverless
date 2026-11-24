//! The status codes a module returns.

/// A status a module returns from a call. Zero succeeds; every failure is
/// negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Success.
    Ok,
    /// The output does not fit; final, the call fails.
    BufferTooSmall,
    /// Malformed input for the point.
    InvalidArgument,
    /// Unknown point or version.
    Unsupported,
    /// The module refused an over-limit input.
    Limit,
    /// The module stopped because `cancelled()` said so.
    Cancelled,
    /// The module failed; optional UTF-8 detail is in the output region.
    Module,
    /// A call arrived before a successful init.
    NotInitialized,
}

impl Status {
    /// The code a module returns for this status.
    pub const fn code(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::BufferTooSmall => -1,
            Self::InvalidArgument => -2,
            Self::Unsupported => -3,
            Self::Limit => -4,
            Self::Cancelled => -5,
            Self::Module => -6,
            Self::NotInitialized => -7,
        }
    }
}

/// The status a returned code stands for.
///
/// # Errors
///
/// The code itself when the contract does not define it, which makes the
/// module malformed.
pub const fn status_of(code: i32) -> Result<Status, i32> {
    match code {
        0 => Ok(Status::Ok),
        -1 => Ok(Status::BufferTooSmall),
        -2 => Ok(Status::InvalidArgument),
        -3 => Ok(Status::Unsupported),
        -4 => Ok(Status::Limit),
        -5 => Ok(Status::Cancelled),
        -6 => Ok(Status::Module),
        -7 => Ok(Status::NotInitialized),
        other => Err(other),
    }
}
