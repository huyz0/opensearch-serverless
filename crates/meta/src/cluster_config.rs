//! The cluster config register: an operator's persistent settings, one
//! register a deployment.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ClusterConfig.java`.
//! Operators write it at human rates, so an update is a read and a
//! compare-and-swap retried a few times without a pause: no lock and no
//! leader. The value is [`ClusterSettings`], which Java nodes read too.

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{ClusterSettings, cluster_config_key};

use crate::error::MetaError;

/// How many times an update reads and swaps before giving up to
/// contention, as the Java's.
const ATTEMPTS: u32 = 10;

/// The cluster config register of one deployment.
pub struct ClusterConfig {
    store: Arc<dyn BlobStore>,
    key: String,
}

/// One read of the register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValue {
    /// The generation read, for a later swap: [`ABSENT_GENERATION`] when
    /// nothing has ever been set.
    pub generation: u64,
    /// The settings it held, empty when nothing has ever been set.
    pub settings: ClusterSettings,
}

impl ClusterConfig {
    /// The register under `base` (empty, or ending in `/`) of `store`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end in
    /// `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Result<Self, MetaError> {
        Ok(Self {
            store,
            key: cluster_config_key(base)?,
        })
    }

    /// Reads the settings, through `buf`: one request.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the store fails or the value does not fit
    /// `buf`; [`MetaError::Format`] for a malformed value.
    pub fn read(&self, buf: &mut [u8]) -> Result<ConfigValue, MetaError> {
        Ok(match self.store.read_register(&self.key, buf)? {
            Some(read) => ConfigValue {
                generation: read.generation,
                settings: ClusterSettings::parse(buf.get(..read.len).unwrap_or_default())?,
            },
            None => ConfigValue {
                generation: ABSENT_GENERATION,
                settings: ClusterSettings::default(),
            },
        })
    }

    /// Applies `changes` in order, a value setting its key and `None`
    /// removing it, merged with what the register holds; returns the
    /// settings written. Each attempt reads and then creates the register,
    /// or swaps it from the generation read; one that loses to another
    /// update reads again, up to 10 attempts, as the Java's.
    ///
    /// # Errors
    ///
    /// [`MetaError::SettingsRefused`], before anything is written, for
    /// changes that make a list, leave a `p.0` beside a `p`, or grow past
    /// [`oss_format::MAX_CLUSTER_SETTINGS_BYTES`]; [`MetaError::Contended`]
    /// when every attempt lost; [`MetaError::Format`] for a malformed value
    /// or a result that does not fit `buf`; [`MetaError::Blob`] when the
    /// store fails or the value read does not fit `buf`.
    pub fn update(
        &self,
        changes: &[(&str, Option<&str>)],
        buf: &mut [u8],
    ) -> Result<ClusterSettings, MetaError> {
        for _ in 0..ATTEMPTS {
            let ConfigValue {
                generation,
                mut settings,
            } = self.read(buf)?;
            settings.apply(changes)?;
            let len = settings.write(buf)?;
            let value = buf.get(..len).unwrap_or_default();
            let outcome = if generation == ABSENT_GENERATION {
                self.store.create_register(&self.key, value)?
            } else {
                self.store.swap_register(&self.key, generation, value)?
            };
            if matches!(outcome, SwapOutcome::Applied(_)) {
                return Ok(settings);
            }
        }
        Err(MetaError::Contended)
    }
}
