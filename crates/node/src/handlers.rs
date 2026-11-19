//! The transport handlers a node serves, each behind the signature check.

use std::sync::Arc;

use oss_transport::auth::{Authenticator, Guarded};
use oss_transport::server::{Handler, Handlers};

/// An action that already has a handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate(pub String);

impl std::fmt::Display for Duplicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a handler is registered for [{}] already", self.0)
    }
}

impl std::error::Error for Duplicate {}

/// What a node serves over its transport, by action.
pub struct Registry {
    auth: Arc<Authenticator>,
    handlers: Handlers,
}

impl Registry {
    /// An empty registry whose handlers verify requests with `auth`.
    #[must_use]
    pub fn new(auth: Arc<Authenticator>) -> Self {
        Self {
            auth,
            handlers: Handlers::new(),
        }
    }

    /// Registers `handler` for `action`, behind the signature check.
    ///
    /// # Errors
    ///
    /// [`Duplicate`] when `action` has a handler; it keeps it.
    pub fn add(&mut self, action: &str, handler: impl Handler + 'static) -> Result<(), Duplicate> {
        if self.handlers.contains_key(action) {
            // ALLOC: one action name, in the error, once.
            return Err(Duplicate(action.to_owned()));
        }
        // ALLOC: one entry an action, at start-up.
        self.handlers.insert(
            action.to_owned(),
            Arc::new(Guarded::new(Arc::clone(&self.auth), handler)),
        );
        Ok(())
    }

    /// The registered actions, sorted.
    #[must_use]
    pub fn actions(&self) -> Vec<&str> {
        // ALLOC: one reference an action, a handful.
        let mut actions: Vec<&str> = self.handlers.keys().map(String::as_str).collect();
        actions.sort_unstable();
        actions
    }

    /// The handlers, for the transport server.
    #[must_use]
    pub fn into_handlers(self) -> Handlers {
        self.handlers
    }
}
