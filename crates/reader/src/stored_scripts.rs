//! Where a script named by `id` finds its source: the node installs one
//! [`StoredScripts`], which reads the stored-script set through its cache.

use std::fmt;
use std::sync::{Arc, RwLock};

/// A stored script: its language and source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// The language the script was stored with.
    pub lang: String,
    /// Its source.
    pub source: String,
}

/// Why a stored script could not be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// No script has the id: a 404.
    Missing,
    /// The script set is larger than the cache may hold: a 429.
    TooBig(String),
    /// The set could not be read: a 500.
    Failed(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("the script does not exist"),
            Self::TooBig(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

/// Resolves a stored script by id.
pub trait StoredScripts: Send + Sync {
    /// The script `id`.
    ///
    /// # Errors
    ///
    /// [`ResolveError`].
    fn resolve(&self, id: &str) -> Result<Stored, ResolveError>;
}

static SCRIPTS: RwLock<Option<Arc<dyn StoredScripts>>> = RwLock::new(None);

/// Installs the node's resolver, replacing one installed before.
pub fn install(scripts: Arc<dyn StoredScripts>) {
    if let Ok(mut slot) = SCRIPTS.write() {
        *slot = Some(scripts);
    }
}

/// The installed resolver, none on a node that has not installed one.
pub fn installed() -> Option<Arc<dyn StoredScripts>> {
    SCRIPTS.read().ok().and_then(|slot| slot.clone())
}
