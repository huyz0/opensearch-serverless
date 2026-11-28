//! Whether the node compiles Painless source itself.

use oss_mem::MemoryBudget;
use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.scripting.painless.native_compile`
/// (`OSS_SCRIPTING_PAINLESS_NATIVE_COMPILE`): whether a Painless script is
/// compiled by this node's Rust front end and run by its engine. Default
/// `false`: a Painless script is refused, as an expression-only node does
/// (docs/adr/0069). Node scope only: the switch is process-wide.
pub const NATIVE_COMPILE: BoolSetting = BoolSetting::new(
    "serverless.scripting.painless.native_compile",
    false,
    Scope::Node,
    "Compile Painless scripts in this node instead of refusing them.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[NATIVE_COMPILE.setting()];

/// With the Painless switch on, reserves the compile cache's bytes from the
/// query-execution pool and turns the cache on; a pool that cannot hold them
/// leaves it off (docs/specs/painless-compile-cache). Returns the budget.
pub(crate) fn installing_compile_cache(budget: &MemoryBudget) -> MemoryBudget {
    oss_reader::painless_cache::remove();
    oss_reader::painless_switch::set_limits(oss_reader::painless_switch::native_compile());
    if !oss_reader::painless_switch::native_compile() {
        return budget.clone();
    }
    if let Ok(reservation) = budget.try_reserve(
        oss_mem::Pool::QueryExecution,
        oss_reader::painless_cache::CACHE_BYTES,
    ) {
        oss_reader::painless_cache::install(Box::new(reservation));
    }
    budget.clone()
}
