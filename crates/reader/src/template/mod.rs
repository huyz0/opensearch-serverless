//! Search templates: mustache.java as OpenSearch's `lang-mustache` module
//! runs it for the `mustache` script language.
//!
//! Ported from `MustacheScriptEngine`, `CustomMustacheFactory` and
//! `CustomReflectionObjectHandler`
//! (modules/lang-mustache/src/main/java/org/opensearch/script/mustache/) at
//! OpenSearch 3.10.0 over `MustacheParser`, `DefaultMustacheVisitor` and the
//! codes of mustache.java 0.9.14. A variable is JSON-escaped unless written
//! `{{{x}}}` or `{{&x}}`; `{{#toJson}}x{{/toJson}}`, `{{#join}}x{{/join}}`,
//! `{{#join delimiter='d'}}x{{/join delimiter='d'}}` and
//! `{{#url}}{{x}}{{/url}}` are the functions. Partials, extends, name blocks,
//! pragmas and dynamic names are not supported; a partial is a template that
//! is not found, as it is in the Java with no resource of that name.

mod exec;
mod members;
mod parse;
mod request;
mod val;
mod visit;

pub use request::{TemplateRequest, read_val};
pub use val::Val;

/// Why a template did not render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateError {
    /// mustache.java's `MustacheException` while compiling: the message the
    /// Java gives.
    Compile(String),
    /// An `IllegalArgumentException` while compiling.
    Illegal(String),
    /// A part of mustache that this node does not support yet.
    Unsupported(String),
    /// A failure while running the template.
    Runtime(String),
    /// The rendered text passed the limit.
    TooLarge(usize),
}

/// Renders `source` over `params`, which is an object, as the `mustache`
/// engine does, in at most `limit` bytes.
///
/// # Errors
///
/// [`TemplateError`] for a template that does not compile, uses what this
/// node does not support, fails to run or renders more than `limit` bytes.
pub fn render(source: &str, params: &Val, limit: usize) -> Result<String, TemplateError> {
    let codes = parse::compile(source, "{{", "}}")?;
    let mut exec = exec::Exec::new(params, limit);
    exec.run(&codes)?;
    Ok(exec.finish())
}

#[cfg(test)]
mod tests;
