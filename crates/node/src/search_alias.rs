//! A search naming an alias of one index.
//!
//! The reader opens concrete indexes only. A request whose `index` path
//! parameter is no open or openable index asks the alias source once; an alias
//! of one index is then served as that index, so every later step, and every
//! hit's `_index`, carries the concrete name. A name that is an index never
//! reaches the source: the open the search makes anyway is the probe.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_mem::Pool;
use oss_meta::{DescriptorStore, Target};
use oss_rest::{Exception, Request, RestError};

use super::{Answer, SearchService};

/// What the members of an alias are read from.
pub trait AliasSource: Send + Sync {
    /// The indexes `name` stands for, in order, when it is an alias; `None`
    /// when it is not one.
    ///
    /// # Errors
    ///
    /// The REST error of a failed register read.
    fn members(&self, name: &str, buf: &mut [u8]) -> Result<Option<Vec<String>>, RestError>;
}

/// The alias registers of a descriptor store.
pub struct RegistryAliases {
    descriptors: DescriptorStore,
}

impl RegistryAliases {
    /// Aliases read from `descriptors`.
    pub fn new(descriptors: DescriptorStore) -> Self {
        Self { descriptors }
    }
}

impl AliasSource for RegistryAliases {
    fn members(&self, name: &str, buf: &mut [u8]) -> Result<Option<Vec<String>>, RestError> {
        let resolved = self.descriptors.resolve(name, buf).map_err(|e| {
            RestError::exception(
                // ALLOC: error path; one message.
                500,
                Exception::new("exception", e.to_string()),
            )
        })?;
        Ok(match resolved.target {
            // ALLOC: one name a member, bounded by the record.
            Target::Alias(alias) => Some(alias.indices.iter().map(|i| i.to_string()).collect()),
            _ => None,
        })
    }
}

/// The bytes one alias read may take.
const READ_BYTES: usize = 64 << 10;

impl<S: BlobStore + 'static> SearchService<S> {
    /// Installs where the members of an alias are read. False when one is
    /// installed already.
    pub fn set_aliases(&self, source: Arc<dyn AliasSource>) -> bool {
        self.aliases.set(source).is_ok()
    }

    /// Points the request's `index` at the concrete index when it names an
    /// alias of one.
    pub(super) fn resolve_alias(&self, req: &mut Request<'_>) -> Result<(), RestError> {
        let Some(name) = req.path_param("index") else {
            return Ok(());
        };
        match self.alias_target(&name)? {
            Some(index) => {
                req.rename_path_param("index", index);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// The one index `name` stands for when it is an alias of one; `None`
    /// when it is an index, no alias, or no source is installed.
    pub(super) fn alias_target(&self, name: &str) -> Result<Option<String>, RestError> {
        let Some(source) = self.aliases.get() else {
            return Ok(None);
        };
        // The open a search makes next; an index that opens is never an alias.
        match self.open_shard(name, 0, false) {
            Err(Answer::Search(e)) if e.status == 404 => {}
            _ => return Ok(None),
        }
        let _grant = self
            .budget
            .try_reserve(Pool::RequestBodies, READ_BYTES)
            .map_err(|_| RestError::illegal_argument("the request-body pool is full"))?;
        // ALLOC: one buffer an alias read, READ_BYTES, reserved above.
        let mut buf = vec![0u8; READ_BYTES];
        let Some(mut members) = source.members(name, &mut buf)? else {
            return Ok(None);
        };
        if members.len() != 1 {
            return Err(RestError::exception(
                501,
                Exception::new(
                    "unsupported_alias_option",
                    // ALLOC: error path; one message.
                    format!(
                        "a search of the alias [{name}] of {} indexes is not supported by this node yet",
                        members.len()
                    ),
                ),
            ));
        }
        Ok(members.pop())
    }
}

/// Installs the alias source over the registers under `base`.
///
/// # Errors
///
/// The store's refusal of the base.
#[cfg(feature = "writer")]
pub(crate) fn install<S: BlobStore + 'static>(
    search: &SearchService<S>,
    store: &Arc<dyn BlobStore>,
    base: &str,
    delete_mode: oss_meta::DeleteMode,
) -> Result<(), String> {
    let descriptors =
        DescriptorStore::new(Arc::clone(store), base, delete_mode).map_err(|e| e.to_string())?;
    search.set_aliases(Arc::new(RegistryAliases::new(descriptors)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_blob::Emulator;
    use oss_format::{AliasRecord, IndexRecord};
    use oss_meta::{AliasClock, AliasMutations, DeleteMode};

    #[test]
    fn the_registry_reads_an_alias_and_knows_an_index_is_none() {
        let store = Arc::new(Emulator::new(1 << 24, 3)) as Arc<dyn BlobStore>;
        let descriptors = DescriptorStore::new(store, "", DeleteMode::Conditional).expect("names");
        let mut buf = vec![0u8; 1 << 16];
        for name in ["a", "b"] {
            descriptors
                .create(&IndexRecord::new(name, "u", 1), 1, &mut buf)
                .expect("created");
        }
        let now = || 1_000;
        let pause = |_: std::time::Duration| {};
        let mut mutations = AliasMutations::new(
            &descriptors,
            AliasClock {
                now_millis: &now,
                pause: &pause,
            },
            30_000,
            1 << 16,
        );
        mutations
            .create_alias(&AliasRecord::new("one", &["a"]))
            .expect("one");
        mutations
            .create_alias(&AliasRecord::new("two", &["a", "b"]))
            .expect("two");
        let aliases = RegistryAliases::new(descriptors);
        let mut members = |name: &str| aliases.members(name, &mut buf).expect("reads");
        assert_eq!(members("one"), Some(vec!["a".to_owned()]));
        assert_eq!(members("two"), Some(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(members("a"), None, "an index is no alias");
        assert_eq!(members("absent"), None);
    }
}
