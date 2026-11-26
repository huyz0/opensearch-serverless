//! The commit staging and logging variables.

/// What a node does with staged commits besides staging them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Logged {
    /// Nothing.
    #[default]
    Off,
    /// Logs each in the node log (`OSS_COMMIT_LOGGED`).
    Write,
    /// Logs each, and opens a shard taken from a predecessor from the newest
    /// it logged (`OSS_COMMIT_OPEN_FROM_LOGGED`).
    Open,
}

impl Logged {
    /// Whether staged commits are logged.
    #[must_use]
    pub fn writes(self) -> bool {
        self != Self::Off
    }

    /// Whether a taken shard opens from a logged commit.
    #[must_use]
    pub fn opens(self) -> bool {
        self == Self::Open
    }
}

/// The checkpoint age when commits are staged (`OSS_COMMIT_STAGING` with
/// `OSS_COMMIT_CHECKPOINT_MS`), and whether staged commits are logged
/// (`OSS_COMMIT_LOGGED`, which needs staging), and whether a taken shard
/// opens from one (`OSS_COMMIT_OPEN_FROM_LOGGED`, which needs logging).
///
/// # Errors
///
/// A message naming a variable out of bounds, or logging without staging.
pub(super) fn read(
    flag: &impl Fn(&str, &str) -> Result<bool, String>,
    bounded: &impl Fn(&str, &str, u64, u64) -> Result<u64, String>,
) -> Result<(Option<u64>, Logged), String> {
    // At least two of the default one-second commit periods, at most half the pack deadline.
    let checkpoint = bounded("OSS_COMMIT_CHECKPOINT_MS", "60000", 2_000, 150_000)?;
    let staging = flag("OSS_COMMIT_STAGING", "false")?.then_some(checkpoint);
    let logged = flag("OSS_COMMIT_LOGGED", "false")?;
    if logged && staging.is_none() {
        return Err("OSS_COMMIT_LOGGED needs OSS_COMMIT_STAGING".to_owned());
    }
    let open = flag("OSS_COMMIT_OPEN_FROM_LOGGED", "false")?;
    if open && !logged {
        return Err("OSS_COMMIT_OPEN_FROM_LOGGED needs OSS_COMMIT_LOGGED".to_owned());
    }
    let mode = match (logged, open) {
        (false, _) => Logged::Off,
        (true, false) => Logged::Write,
        (true, true) => Logged::Open,
    };
    Ok((staging, mode))
}
