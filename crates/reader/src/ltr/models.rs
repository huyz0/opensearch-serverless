//! Where a search finds a model: the node installs one [`Models`], which
//! reads the store (through its cache) and hands back what a rescore needs.

use std::fmt;
use std::sync::{Arc, RwLock};

use super::Evaluator;

/// One feature of a model's feature set.
#[derive(Debug)]
pub struct Feature {
    /// Its name.
    pub name: String,
    /// The names the template needs among a query's `params`.
    pub params: Vec<String>,
    /// The template's source: a query object as JSON text, or a string.
    pub template: String,
}

/// A model with the feature set it scores.
#[derive(Debug)]
pub struct Resolved {
    /// The parsed model.
    pub evaluator: Evaluator,
    /// The features, in the order the model reads them.
    pub features: Vec<Feature>,
}

/// Why a model could not be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The store or the model does not exist: a 404.
    NotFound(String),
    /// The model is larger than the cache may hold now: a 429.
    TooBig(String),
    /// The store failed: a 500.
    Failed(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(m) | Self::TooBig(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

/// Resolves a model by store and name.
pub trait Models: Send + Sync {
    /// The model `name` of `store` (the default store when `None`).
    ///
    /// # Errors
    ///
    /// [`ResolveError`].
    fn resolve(&self, store: Option<&str>, name: &str) -> Result<Arc<Resolved>, ResolveError>;

    /// The most hits an `sltr` rescore scores (`serverless.ltr.max_window`).
    fn max_window(&self) -> usize;
}

static MODELS: RwLock<Option<Arc<dyn Models>>> = RwLock::new(None);

/// Installs the node's resolver, replacing one installed before.
pub fn install(models: Arc<dyn Models>) {
    if let Ok(mut slot) = MODELS.write() {
        *slot = Some(models);
    }
}

/// The installed resolver, none while learning to rank is off.
pub fn installed() -> Option<Arc<dyn Models>> {
    MODELS.read().ok().and_then(|slot| slot.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(usize);

    impl Models for Fixed {
        fn resolve(&self, _: Option<&str>, _: &str) -> Result<Arc<Resolved>, ResolveError> {
            Err(ResolveError::NotFound(String::new()))
        }

        fn max_window(&self) -> usize {
            self.0
        }
    }

    #[test]
    fn an_installed_resolver_is_found_and_replaced() {
        install(Arc::new(Fixed(7)));
        assert_eq!(installed().map(|m| m.max_window()), Some(7));
        install(Arc::new(Fixed(9)));
        let m = installed().expect("installed");
        assert_eq!(m.max_window(), 9);
        assert!(matches!(
            m.resolve(None, "x"),
            Err(ResolveError::NotFound(_))
        ));
    }
}
