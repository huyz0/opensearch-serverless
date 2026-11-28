//! The switches of the ported plugins that are off by default, read from the environment.

/// Which of the ported plugins' routes a node serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    /// `serverless.ism.enabled` (`OSS_ISM`): serve the ISM policy routes.
    pub ism: bool,
    /// `serverless.ad.enabled` (`OSS_AD`): serve the anomaly detection detector routes.
    pub ad: bool,
    /// `serverless.observability.enabled` (`OSS_OBSERVABILITY`): serve the observability object routes.
    pub observability: bool,
}

/// Reads each switch, every one off unless set.
pub(super) fn read(flag: &impl Fn(&str, &str) -> Result<bool, String>) -> Result<Switches, String> {
    Ok(Switches {
        ism: flag("OSS_ISM", "false")?,
        ad: flag("OSS_AD", "false")?,
        observability: flag("OSS_OBSERVABILITY", "false")?,
    })
}
