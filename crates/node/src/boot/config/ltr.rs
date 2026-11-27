//! The learning to rank bounds read from the environment.

/// What the model cache and the `sltr` window are bounded by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// `OSS_LTR_MAX_WINDOW`: the most hits an `sltr` rescore scores.
    pub max_window: usize,
    /// `OSS_LTR_CACHE_TTL`, in seconds.
    pub cache_ttl_secs: u64,
    /// `OSS_LTR_CACHE_BYTES`: what the model cache may hold.
    pub cache_bytes: usize,
}

/// Reads each bound in its range.
pub(super) fn read(
    bounded: &impl Fn(&str, &str, u64, u64) -> Result<u64, String>,
) -> Result<Bounds, String> {
    let window = bounded("OSS_LTR_MAX_WINDOW", "1000", 1, 10_000)?;
    let bytes = bounded("OSS_LTR_CACHE_BYTES", "67108864", 1 << 20, 4 << 30)?;
    Ok(Bounds {
        max_window: usize::try_from(window).unwrap_or(1_000),
        cache_ttl_secs: bounded("OSS_LTR_CACHE_TTL", "30", 1, 3_600)?,
        cache_bytes: usize::try_from(bytes).unwrap_or(64 << 20),
    })
}
