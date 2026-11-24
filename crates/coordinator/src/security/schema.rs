//! The keys each kind accepts, and what each holds.

use super::Kind;

/// What a key's value must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    /// A string.
    Text,
    /// An array of strings.
    Strings,
    /// An object with any members.
    Object,
    /// An array of objects with the given keys.
    Entries(&'static [(&'static str, Shape)]),
}

pub(super) type Keys = &'static [(&'static str, Shape)];

const INDEX_ENTRY: Keys = &[
    ("index_patterns", Shape::Strings),
    ("allowed_actions", Shape::Strings),
    ("fls", Shape::Strings),
    ("masked_fields", Shape::Strings),
    ("dls", Shape::Text),
];
const TENANT_ENTRY: Keys = &[
    ("tenant_patterns", Shape::Strings),
    ("allowed_actions", Shape::Strings),
];

/// The keys a body of `kind` may carry.
pub(super) const fn keys(kind: Kind) -> Keys {
    match kind {
        Kind::Roles => &[
            ("cluster_permissions", Shape::Strings),
            ("index_permissions", Shape::Entries(INDEX_ENTRY)),
            ("tenant_permissions", Shape::Entries(TENANT_ENTRY)),
            ("description", Shape::Text),
        ],
        Kind::RolesMapping => &[
            ("users", Shape::Strings),
            ("backend_roles", Shape::Strings),
            ("hosts", Shape::Strings),
            ("and_backend_roles", Shape::Strings),
            ("description", Shape::Text),
        ],
        Kind::InternalUsers => &[
            ("password", Shape::Text),
            ("hash", Shape::Text),
            ("backend_roles", Shape::Strings),
            ("opendistro_security_roles", Shape::Strings),
            ("attributes", Shape::Object),
            ("description", Shape::Text),
        ],
        Kind::Tenants => &[("description", Shape::Text)],
        Kind::ActionGroups => &[
            ("allowed_actions", Shape::Strings),
            ("type", Shape::Text),
            ("description", Shape::Text),
        ],
    }
}

/// The keys a body must carry.
pub(super) const fn mandatory(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::ActionGroups => &["allowed_actions"],
        _ => &[],
    }
}

/// A mapping names at least one of these.
pub(super) const MAPPING_ONE_OF: &[&str] =
    &["backend_roles", "hosts", "and_backend_roles", "users"];
