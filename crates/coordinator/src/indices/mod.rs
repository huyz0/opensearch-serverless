//! Index administration decisions: what a create, settings or alias request
//! means before any node acts on it.

pub mod admin;
pub mod create;
pub mod create_inherit;
pub mod flat;
pub mod mapping_update;
pub mod rollover;
pub mod settings_update;
pub mod time_value;
