//! The commit staging and logging variables.

/// The checkpoint age when commits are staged (`OSS_COMMIT_STAGING` with
/// `OSS_COMMIT_CHECKPOINT_MS`), and whether staged commits are logged
/// (`OSS_COMMIT_LOGGED`, which needs staging).
///
/// # Errors
///
/// A message naming a variable out of bounds, or logging without staging.
pub(super) fn read(
    flag: &impl Fn(&str, &str) -> Result<bool, String>,
    bounded: &impl Fn(&str, &str, u64, u64) -> Result<u64, String>,
) -> Result<(Option<u64>, bool), String> {
    // At least two of the default one-second commit periods, at most half the pack deadline.
    let checkpoint = bounded("OSS_COMMIT_CHECKPOINT_MS", "60000", 2_000, 150_000)?;
    let staging = flag("OSS_COMMIT_STAGING", "false")?.then_some(checkpoint);
    let logged = flag("OSS_COMMIT_LOGGED", "false")?;
    if logged && staging.is_none() {
        return Err("OSS_COMMIT_LOGGED needs OSS_COMMIT_STAGING".to_owned());
    }
    Ok((staging, logged))
}
