//! The node binary's composition root.
//!
//! Each role is compiled in only when its build switch (a Cargo feature) is
//! on, so a reader-only build carries no writer code. [`enabled_roles`] reports
//! what this binary was built with.

#[cfg(feature = "writer")]
pub mod bulk_route;
#[cfg(feature = "writer")]
pub mod forward;
#[cfg(any(feature = "reader", feature = "writer", feature = "coordinator"))]
pub mod handlers;
#[cfg(feature = "writer")]
pub mod newest;
#[cfg(feature = "reader")]
pub mod newest_ask;
#[cfg(feature = "reader")]
pub mod open_indexes;
#[cfg(feature = "reader")]
pub mod overlay_builder;
#[cfg(feature = "reader")]
pub mod push;
#[cfg(feature = "writer")]
pub mod push_link;
#[cfg(feature = "writer")]
pub mod push_sender;
#[cfg(feature = "reader")]
pub mod search;
pub mod settings;
#[cfg(feature = "reader")]
pub mod tail_held;
#[cfg(feature = "writer")]
pub mod tail_keep;
#[cfg(feature = "writer")]
pub mod tail_link;
#[cfg(feature = "reader")]
pub mod tail_overlay;
#[cfg(feature = "writer")]
pub mod tail_push;
#[cfg(feature = "reader")]
pub mod tail_reader_link;
#[cfg(feature = "writer")]
pub mod tail_serve;
#[cfg(feature = "writer")]
pub mod writer;

/// A role a node can serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Serves searches from published segments (`reader` feature).
    Reader,
    /// Makes writes durable and publishes segments (`writer` feature).
    Writer,
    /// Routes, fans out and reduces requests (`coordinator` feature).
    Coordinator,
}

impl Role {
    /// The role's name, as used in configuration and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Writer => "writer",
            Role::Coordinator => "coordinator",
        }
    }
}

/// The roles this binary was built with, in a fixed order.
pub fn enabled_roles() -> Vec<Role> {
    let mut roles = Vec::with_capacity(3);
    if cfg!(feature = "reader") {
        roles.push(Role::Reader);
    }
    if cfg!(feature = "writer") {
        roles.push(Role::Writer);
    }
    if cfg!(feature = "coordinator") {
        roles.push(Role::Coordinator);
    }
    roles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_follow_the_build_switches() {
        let roles = enabled_roles();
        assert_eq!(roles.contains(&Role::Reader), cfg!(feature = "reader"));
        assert_eq!(roles.contains(&Role::Writer), cfg!(feature = "writer"));
        assert_eq!(
            roles.contains(&Role::Coordinator),
            cfg!(feature = "coordinator")
        );
    }

    #[test]
    fn names_are_distinct() {
        let names = [Role::Reader, Role::Writer, Role::Coordinator].map(Role::name);
        assert_eq!(names, ["reader", "writer", "coordinator"]);
    }
}
