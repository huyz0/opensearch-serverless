//! The SQL bounds read from the environment.

/// What a SQL statement and its request are bounded by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// `OSS_SQL_SIZE_LIMIT`: the most rows a statement returns.
    pub size_limit: usize,
    /// `OSS_SQL_MAX_REQUEST_BYTES`: the largest request body.
    pub max_request_bytes: usize,
}

/// Reads each bound in its range.
pub(super) fn read(
    bounded: &impl Fn(&str, &str, u64, u64) -> Result<u64, String>,
) -> Result<Bounds, String> {
    let size = bounded("OSS_SQL_SIZE_LIMIT", "200", 1, 10_000)?;
    let bytes = bounded("OSS_SQL_MAX_REQUEST_BYTES", "1048576", 1_024, 16 << 20)?;
    Ok(Bounds {
        size_limit: usize::try_from(size).unwrap_or(200),
        max_request_bytes: usize::try_from(bytes).unwrap_or(1 << 20),
    })
}
