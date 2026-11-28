//! What the runner asks of the node: the effects its actions have. A trait,
//! so the decision is tested with a fake and the node supplies the real one
//! over its own routes.

/// The node's side of the actions. An error is the reason a step failed.
pub trait Backend: Send + Sync {
    /// Blocks writes to `index` (`on`) or lifts the block.
    ///
    /// # Errors
    ///
    /// The reason the block could not be set.
    fn set_read_only(&self, index: &str, on: bool) -> Result<(), String>;

    /// Sets `index.number_of_replicas` of `index` to `n`.
    ///
    /// # Errors
    ///
    /// The reason the setting could not be changed.
    fn set_replicas(&self, index: &str, n: u32) -> Result<(), String>;

    /// Deletes `index`, as `DELETE /{index}` does.
    ///
    /// # Errors
    ///
    /// The reason the index could not be deleted.
    fn delete(&self, index: &str) -> Result<(), String>;
}

/// A backend that can do nothing: every action it is asked for fails.
pub struct Unsupported;

impl Backend for Unsupported {
    fn set_read_only(&self, _index: &str, _on: bool) -> Result<(), String> {
        Err("the node cannot set an index block yet".to_owned())
    }

    fn set_replicas(&self, _index: &str, _n: u32) -> Result<(), String> {
        Err("the node cannot set replicas yet".to_owned())
    }

    fn delete(&self, _index: &str) -> Result<(), String> {
        Err("the node cannot delete an index yet".to_owned())
    }
}
