//! The switches of the plugins ported natively, read from the environment.

/// Which plugin routes this node serves; each is off by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    /// `serverless.ism.enabled` (`OSS_ISM`): the index state management policy routes.
    pub ism: bool,
    /// `serverless.ml.enabled` (`OSS_ML`): the ML Commons registry routes.
    pub ml: bool,
}

/// Reads each switch.
pub(super) fn read(flag: &impl Fn(&str, &str) -> Result<bool, String>) -> Result<Switches, String> {
    Ok(Switches {
        ism: flag("OSS_ISM", "false")?,
        ml: flag("OSS_ML", "false")?,
    })
}
