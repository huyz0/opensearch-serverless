//! Ingest pipelines: compiled from a stored definition and run over one
//! document's source.
//!
//! The processors are the ones of the ingest-common module with the options
//! they take here; an option this engine does not run is refused when the
//! pipeline is compiled, so a stored pipeline never silently ignores part of
//! itself. Fields are dotted paths into nested objects.

use oss_doc::mapping::json::{Json, parse};

/// The processors a pipeline may name.
pub const PROCESSORS: &[&str] = &[
    "append",
    "convert",
    "lowercase",
    "remove",
    "rename",
    "set",
    "trim",
    "uppercase",
];

/// The deepest `on_failure` nesting.
const MAX_DEPTH: usize = 8;

/// Options every processor takes.
const COMMON: &[&str] = &["tag", "description", "ignore_failure", "on_failure"];

/// A compiled pipeline.
#[derive(Debug, Clone)]
pub struct Pipeline {
    processors: Vec<Processor>,
    on_failure: Vec<Processor>,
}

mod compile;
mod run;
pub(crate) mod script;

#[derive(Debug, Clone)]
pub(super) struct Processor {
    pub(super) step: Step,
    pub(super) ignore_failure: bool,
    pub(super) on_failure: Vec<Processor>,
}

#[derive(Debug, Clone)]
pub(super) enum Step {
    Set {
        field: String,
        value: Json,
        overwrite: bool,
        ignore_empty: bool,
    },
    Remove {
        fields: Vec<String>,
        ignore_missing: bool,
    },
    Rename {
        field: String,
        target: String,
        ignore_missing: bool,
    },
    Text {
        field: String,
        target: String,
        ignore_missing: bool,
        op: TextOp,
    },
    Append {
        field: String,
        values: Vec<Json>,
        allow_duplicates: bool,
    },
    Convert {
        field: String,
        target: String,
        ignore_missing: bool,
        to: Type,
    },
    Script(script::Script),
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TextOp {
    Lower,
    Upper,
    Trim,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Type {
    Integer,
    Long,
    Float,
    Double,
    Boolean,
    Text,
}

impl Pipeline {
    /// Compiles a stored definition.
    ///
    /// # Errors
    ///
    /// The reason a definition cannot be run: not an object, no
    /// `processors`, a processor of a type or with an option this engine does
    /// not run, or a field it requires missing.
    pub fn compile(definition: &[u8]) -> Result<Self, String> {
        let Ok(parsed @ Json::Obj(_)) = parse(definition) else {
            // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
            return Err("the pipeline is not a JSON object".to_owned());
        };
        let processors = compile::list(
            parsed
                .get("processors")
                .ok_or("[processors] required property is missing")?,
            0,
        )?;
        let on_failure = match parsed.get("on_failure") {
            Some(handlers) => compile::list(handlers, 1)?,
            // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
            None => Vec::new(),
        };
        Ok(Self {
            processors,
            on_failure,
        })
    }

    /// Runs the pipeline over a document's source and answers the new source.
    ///
    /// # Errors
    ///
    /// The reason the document failed: a source that is not an object, or a
    /// processor that failed with no `on_failure` or `ignore_failure` to
    /// take it.
    pub fn run(&self, source: &[u8]) -> Result<Vec<u8>, String> {
        let Ok(mut doc @ Json::Obj(_)) = parse(source) else {
            // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
            return Err("the document source is not a JSON object".to_owned());
        };
        if let Err(reason) = run::run_all(&self.processors, &mut doc) {
            if self.on_failure.is_empty() {
                return Err(reason);
            }
            run::run_all(&self.on_failure, &mut doc)?;
        }
        Ok(doc.to_json().into_bytes())
    }
}
