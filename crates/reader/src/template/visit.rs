//! The visitor side of the compiler: the codes `CustomMustacheVisitor` builds
//! for `toJson`, `join` and `url`, and the check of partials once a template
//! has compiled.

use super::TemplateError;
use super::parse::{Func, Kind, Node};

/// `CustomMustacheFactory.CustomCode.extractVariableName`: the one code of
/// `body`, as text.
pub(super) fn function_name(function: &str, body: &[Node]) -> Result<String, TemplateError> {
    let [only] = body else {
        // ALLOC: the template, its params and its output, which the request limits bound.
        return Err(TemplateError::Compile(format!(
            "Mustache function [{function}] must contain one and only one identifier"
        )));
    };
    Ok(match only.kind {
        Kind::Write => only.appended.clone(),
        _ => {
            // ALLOC: the template, its params and its output, which the request limits bound.
            let mut out = String::new();
            only.identity(&mut out);
            out
        }
    })
}

/// `join delimiter='...'`: the delimiter of `CustomJoinerCode`'s pattern
/// `^(?:join delimiter='(.*)')$`.
pub(super) fn custom_delimiter(variable: &str) -> Option<&str> {
    let rest = variable.strip_prefix("join delimiter='")?;
    let delimiter = rest.strip_suffix('\'')?;
    let line_end = |c: char| matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}');
    (!delimiter.chars().any(line_end)).then_some(delimiter)
}

/// What `Mustache.init` finds once a template has compiled: the first
/// partial or extended template, in order, which does not exist.
pub(super) fn check_partials(codes: &[Node]) -> Result<(), TemplateError> {
    for node in codes {
        match &node.kind {
            Kind::Partial { name } => {
                if name.trim().starts_with('*') {
                    return Err(unsupported("a dynamic partial"));
                }
                // ALLOC: the template, its params and its output, which the request limits bound.
                return Err(TemplateError::Compile(format!("Template {name} not found")));
            }
            Kind::Extend { name, body } => {
                // `ExtendCode.init` takes only blocks, text and comments.
                if let Some(illegal) = body
                    .iter()
                    .find(|n| !matches!(n.kind, Kind::Write | Kind::Comment { .. }))
                {
                    // ALLOC: the template, its params and its output, which the request limits bound.
                    return Err(TemplateError::Illegal(format!(
                        "Illegal code in extend section: {}",
                        illegal.java_class()
                    )));
                }
                // ALLOC: the template, its params and its output, which the request limits bound.
                return Err(TemplateError::Compile(format!("Template {name} not found")));
            }
            Kind::Section { body, .. } | Kind::Function { body, .. } | Kind::Url { body } => {
                check_partials(body)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// The codes `MustacheVisitor` collects.
pub(super) struct Visitor(pub(super) Vec<Node>);

impl Visitor {
    pub(super) fn write(&mut self, text: &str, sm: &[char], em: &[char]) {
        if text.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(last) => last.appended.push_str(text),
            None => self.0.push(Node {
                kind: Kind::Write,
                // ALLOC: the template's text, bounded by the template.
                appended: text.to_owned(),
                sm: sm.iter().collect(),
                em: em.iter().collect(),
                line: 0,
            }),
        }
    }

    pub(super) fn push(&mut self, kind: Kind, line: i64, sm: &[char], em: &[char]) {
        self.0.push(Node {
            kind,
            // ALLOC: the template, its params and its output, which the request limits bound.
            appended: String::new(),
            // ALLOC: the template, its params and its output, which the request limits bound.
            sm: sm.iter().collect(),
            // ALLOC: the template, its params and its output, which the request limits bound.
            em: em.iter().collect(),
            line,
        });
    }
}

pub(super) fn unsupported(what: &str) -> TemplateError {
    // ALLOC: the template, its params and its output, which the request limits bound.
    TemplateError::Unsupported(format!("{what} is not supported by this node yet"))
}

pub(super) fn reject_dynamic(name: &str) -> Result<(), TemplateError> {
    if name.starts_with('*') {
        Err(unsupported("a dynamic name"))
    } else {
        Ok(())
    }
}

/// `CustomMustacheVisitor.iterable`.
pub(super) fn iterable_code(
    mv: &mut Visitor,
    variable: &str,
    body: Vec<Node>,
    line: i64,
    sm: &[char],
    em: &[char],
) -> Result<(), TemplateError> {
    if variable.eq_ignore_ascii_case("toJson") {
        let name = function_name("toJson", &body)?;
        reject_dynamic(&name)?;
        mv.push(
            Kind::Function {
                func: Func::ToJson,
                name,
                body,
            },
            line,
            sm,
            em,
        );
    } else if variable.eq_ignore_ascii_case("join") {
        let name = function_name("join", &body)?;
        reject_dynamic(&name)?;
        mv.push(
            Kind::Function {
                // ALLOC: the template, its params and its output, which the request limits bound.
                func: Func::Join(",".to_owned(), false),
                name,
                body,
            },
            line,
            sm,
            em,
        );
    } else if let Some(delimiter) = custom_delimiter(variable) {
        let name = function_name("join", &body)?;
        reject_dynamic(&name)?;
        mv.push(
            Kind::Function {
                // ALLOC: the template's delimiter, bounded by the template.
                func: Func::Join(delimiter.to_owned(), true),
                name,
                body,
            },
            line,
            sm,
            em,
        );
    } else if variable.eq_ignore_ascii_case("url") {
        mv.push(Kind::Url { body }, line, sm, em);
    } else {
        reject_dynamic(variable)?;
        mv.push(
            Kind::Section {
                // ALLOC: the template's name, bounded by the template.
                name: variable.to_owned(),
                body,
                inverted: false,
            },
            line,
            sm,
            em,
        );
    }
    Ok(())
}
