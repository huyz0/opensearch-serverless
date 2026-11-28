//! The switches of the plugins ported natively, read from the environment.

/// Which plugin routes this node serves; ISM, AD, ML, observability and reports are on by default, `OSS_*=false` turns one off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    /// `serverless.ism.enabled` (`OSS_ISM`): the index state management policy routes.
    pub ism: bool,
    /// `serverless.ad.enabled` (`OSS_AD`): the anomaly detection detector routes.
    pub ad: bool,
    /// `serverless.ml.enabled` (`OSS_ML`): the ML Commons registry routes.
    pub ml: bool,
    /// `serverless.async_search.enabled` (`OSS_ASYNC_SEARCH`): the asynchronous search routes.
    pub async_search: bool,
    /// `serverless.observability.enabled` (`OSS_OBSERVABILITY`): the observability object routes.
    pub observability: bool,
    /// `serverless.reports.enabled` (`OSS_REPORTS`): the report definition routes.
    pub reports: bool,
}

/// Reads each switch.
pub(super) fn read(flag: &impl Fn(&str, &str) -> Result<bool, String>) -> Result<Switches, String> {
    Ok(Switches {
        ism: flag("OSS_ISM", "true")?,
        ad: flag("OSS_AD", "true")?,
        ml: flag("OSS_ML", "true")?,
        async_search: flag("OSS_ASYNC_SEARCH", "true")?,
        observability: flag("OSS_OBSERVABILITY", "true")?,
        reports: flag("OSS_REPORTS", "true")?,
    })
}
