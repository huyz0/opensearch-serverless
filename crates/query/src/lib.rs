//! SQL for the serverless node: lexer and parser for the plugin's dialect, the lowering
//! of a `SELECT` to a search body, and the `jdbc`, `csv` and `raw` result writers.
//!
//! The crate does no I/O. The caller supplies names, mappings and search answers, and
//! sends the search body it gets back through the coordinator's search path
//! ([ADR 0082](../../../docs/adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md)).

mod ast;
mod error;
mod lexer;
mod mapping;
mod meta;
mod parser;
mod parser_pred;
mod plan;
mod result;

pub use ast::{CmpOp, Item, Lit, Order, Pred, Relevance, Select, Statement};
pub use error::Error;
pub use mapping::{Field, Mapping};
pub use meta::{describe, like, show_tables};
pub use parser::parse;
pub use plan::{Column, Plan, plan};
pub use result::{Format, rows, write};

/// The bounds every parse enforces before it allocates in proportion to the input.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The most bytes a statement may have.
    pub max_statement_bytes: usize,
    /// The most tokens a statement may have.
    pub max_tokens: usize,
    /// The deepest nesting of parentheses and `NOT`.
    pub max_depth: usize,
    /// The longest identifier or word.
    pub max_ident: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_statement_bytes: 64 * 1024,
            max_tokens: 4096,
            max_depth: 64,
            max_ident: 255,
        }
    }
}
