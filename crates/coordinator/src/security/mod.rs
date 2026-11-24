//! The security configuration the management API stores and answers: five
//! kinds of named document, each validated as the stock security plugin
//! 3.10.0 validates it and rendered as it renders it.
//!
//! The expected answers are `fixtures/compat/security/`, recorded from the
//! real plugin. A document is stored as the request sent it, once valid
//! (an internal user's `password` replaced by its `hash` by the caller); the
//! defaults the plugin adds on a read are added when it is rendered.
//! Decisions of authentication and authorization live in sibling modules.

mod render;
mod schema;
mod validate;

pub use render::{deleted, missing, render, saved};
pub use validate::{
    MIN_PASSWORD_CHARS, needs_credentials, password_ok, validate, weak_password, with_hash,
};

/// Which kind of document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A role: cluster, index and tenant permissions.
    Roles,
    /// A role mapping: users, backend roles and hosts a role is granted to.
    RolesMapping,
    /// An internal user: a password hash, backend roles and attributes.
    InternalUsers,
    /// A tenant.
    Tenants,
    /// An action group: a named set of actions.
    ActionGroups,
}

impl Kind {
    /// Every kind, in the order the API lists them.
    pub const ALL: [Self; 5] = [
        Self::Roles,
        Self::RolesMapping,
        Self::InternalUsers,
        Self::Tenants,
        Self::ActionGroups,
    ];

    /// How the plugin names one document of the kind in its messages.
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Roles => "role",
            Self::RolesMapping => "rolesmapping",
            Self::InternalUsers => "user",
            Self::Tenants => "tenant",
            Self::ActionGroups => "actiongroup",
        }
    }
}

/// An answer to a management request: a status and a JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The JSON body.
    pub body: String,
}
