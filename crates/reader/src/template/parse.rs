//! The template compiler: `MustacheParser.compile` and the
//! `DefaultMustacheVisitor` and `CustomMustacheVisitor` that build its codes,
//! from mustache.java 0.9.14 as OpenSearch's `lang-mustache` module runs it.

use super::TemplateError;
use super::visit::{Visitor, check_partials, iterable_code, reject_dynamic, unsupported};

/// The file name the engine compiles a template under.
const FILE: &str = "query-template";

/// The deepest nesting of sections a template may have: Java's stack ends a
/// deeper one with a `StackOverflowError`.
pub(super) const MAX_DEPTH: usize = 256;

/// One compiled code, with the text that follows it up to the next tag.
#[derive(Clone, Debug)]
pub(super) struct Node {
    pub(super) kind: Kind,
    /// Text written after the code runs.
    pub(super) appended: String,
    /// The delimiters in effect where the code stands.
    pub(super) sm: String,
    pub(super) em: String,
    /// The line the code stands on.
    pub(super) line: i64,
}

/// What a code does.
#[derive(Clone, Debug)]
pub(super) enum Kind {
    /// Text only.
    Write,
    /// `{{name}}`, `{{{name}}}` or `{{&name}}`.
    Value { name: String, encoded: bool },
    /// `{{#name}}...{{/name}}` or `{{^name}}...{{/name}}`.
    Section {
        name: String,
        body: Vec<Node>,
        inverted: bool,
    },
    /// `{{#toJson}}name{{/toJson}}` or `{{#join}}name{{/join}}`.
    Function {
        func: Func,
        name: String,
        body: Vec<Node>,
    },
    /// `{{#url}}...{{/url}}`.
    Url { body: Vec<Node> },
    /// `{{! text }}`.
    Comment { text: String },
    /// `{{>name}}`: a template that is not found, reported once the whole
    /// template has compiled, as the Java loads partials when it initialises.
    Partial { name: String },
    /// `{{<name}}...{{/name}}`: likewise.
    Extend { name: String, body: Vec<Node> },
    /// The end of the template.
    Eof,
}

/// The functions that take a variable's name as their text.
#[derive(Clone, Debug)]
pub(super) enum Func {
    ToJson,
    /// The delimiter, and whether it was written `join delimiter='d'`.
    Join(String, bool),
}

/// Java's `String.trim()`: the characters up to and including a space.
pub(super) fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

fn compile_error(message: impl Into<String>, line: i64) -> TemplateError {
    // ALLOC: the template, its params and its output, which the request limits bound.
    TemplateError::Compile(format!("{} @[{FILE}:{line}]", message.into()))
}

/// The text of `codes` as `Code.identity` writes it.
pub(super) fn identity(codes: &[Node], out: &mut String) {
    for node in codes {
        node.identity(out);
    }
}

impl Node {
    pub(super) fn identity(&self, out: &mut String) {
        match &self.kind {
            Kind::Write | Kind::Eof => {}
            Kind::Value { name, encoded } => {
                out.push_str(&self.sm);
                if !*encoded {
                    out.push('{');
                }
                out.push_str(name);
                if !*encoded {
                    out.push('}');
                }
                out.push_str(&self.em);
            }
            Kind::Section {
                name,
                body,
                inverted,
            } => {
                self.tag(out, if *inverted { "^" } else { "#" }, name);
                identity(body, out);
                self.tag(out, "/", name);
            }
            Kind::Function { func, body, .. } => {
                let code = match func {
                    Func::ToJson => "toJson",
                    Func::Join(..) => "join",
                };
                self.tag(out, "#", code);
                identity(body, out);
                self.tag(out, "/", code);
            }
            Kind::Url { body } => {
                identity(body, out);
                return;
            }
            Kind::Comment { text } => self.tag(out, "!", text),
            Kind::Partial { name } => self.tag(out, ">", name),
            Kind::Extend { name, body } => {
                self.tag(out, "<", name);
                identity(body, out);
                self.tag(out, "/", name);
            }
        }
        out.push_str(&self.appended);
    }

    /// The Java class of the code.
    pub(super) fn java_class(&self) -> &'static str {
        match &self.kind {
            Kind::Write => "com.github.mustachejava.codes.WriteCode",
            Kind::Eof => "com.github.mustachejava.DefaultMustacheVisitor$1",
            Kind::Value { .. } => "com.github.mustachejava.codes.ValueCode",
            Kind::Section {
                inverted: false, ..
            } => "com.github.mustachejava.codes.IterableCode",
            Kind::Section { inverted: true, .. } => "com.github.mustachejava.codes.NotIterableCode",
            Kind::Function {
                func: Func::ToJson, ..
            } => "org.opensearch.script.mustache.CustomMustacheFactory$ToJsonCode",
            Kind::Function {
                func: Func::Join(_, false),
                ..
            } => "org.opensearch.script.mustache.CustomMustacheFactory$JoinerCode",
            Kind::Function {
                func: Func::Join(_, true),
                ..
            } => "org.opensearch.script.mustache.CustomMustacheFactory$CustomJoinerCode",
            Kind::Url { .. } => {
                "org.opensearch.script.mustache.CustomMustacheFactory$UrlEncoderCode"
            }
            Kind::Comment { .. } => "com.github.mustachejava.codes.CommentCode",
            Kind::Partial { .. } => "com.github.mustachejava.codes.PartialCode",
            Kind::Extend { .. } => "com.github.mustachejava.codes.ExtendCode",
        }
    }

    fn tag(&self, out: &mut String, tag: &str, name: &str) {
        out.push_str(&self.sm);
        out.push_str(tag);
        out.push_str(name);
        out.push_str(&self.em);
    }
}

pub(super) struct Parser {
    src: Vec<char>,
    pos: usize,
    line: i64,
}

/// What `compile` reads a template with.
pub(super) fn compile(source: &str, sm: &str, em: &str) -> Result<Vec<Node>, TemplateError> {
    let mut parser = Parser {
        // ALLOC: one char a character of the template, which the request's body bounds.
        src: source.chars().collect(),
        pos: 0,
        line: 0,
    };
    // ALLOC: the template, its params and its output, which the request limits bound.
    let sm: Vec<char> = sm.chars().collect();
    // ALLOC: the template, its params and its output, which the request limits bound.
    let em: Vec<char> = em.chars().collect();
    let codes = parser.compile(None, sm, em, true, 0)?;
    check_partials(&codes)?;
    Ok(codes)
}

impl Parser {
    fn read(&mut self) -> Option<char> {
        let c = self.src.get(self.pos).copied();
        if c.is_some() {
            self.pos = self.pos.saturating_add(1);
        }
        c
    }

    #[allow(clippy::too_many_lines)]
    fn compile(
        &mut self,
        tag: Option<&str>,
        mut sm: Vec<char>,
        mut em: Vec<char>,
        mut start_of_line: bool,
        depth: usize,
    ) -> Result<Vec<Node>, TemplateError> {
        if depth > MAX_DEPTH {
            // ALLOC: the template, its params and its output, which the request limits bound.
            return Err(unsupported(&format!(
                "sections nested more than [{MAX_DEPTH}] deep"
            )));
        }
        let mut saw_cr = false;
        let start_line = self.line;
        // ALLOC: the template, its params and its output, which the request limits bound.
        let mut mv = Visitor(Vec::new());
        let mut only_whitespace = true;
        let mut iterable = self.line != 0;
        if self.line == 0 {
            self.line = 1;
        }
        // ALLOC: the template, its params and its output, which the request limits bound.
        let mut out = String::new();
        let mut n: i64 = 0;
        while let Some(c) = self.read() {
            n = n.saturating_add(1);
            if c == '\r' {
                saw_cr = true;
                continue;
            }
            if c == '\n' {
                self.line = self.line.saturating_add(1);
                if !iterable || !only_whitespace {
                    if saw_cr {
                        out.push('\r');
                    }
                    out.push('\n');
                }
                mv.write(&out, &sm, &em);
                out.clear();
                iterable = false;
                only_whitespace = true;
                start_of_line = true;
                n = 0;
                continue;
            }
            saw_cr = false;
            if sm.first() == Some(&c) {
                let mark = self.pos;
                if sm.len() == 1 || self.read() == sm.get(1).copied() {
                    // ALLOC: the template, its params and its output, which the request limits bound.
                    let mut sb = String::new();
                    let mark2 = self.pos;
                    let first = self.read();
                    n = n.saturating_add(1);
                    let mut delimiter = first == Some('=');
                    if delimiter {
                        sb.push('=');
                    } else {
                        self.pos = mark2;
                        n = n.saturating_sub(1);
                    }
                    let mut at_eof = true;
                    while let Some(c) = self.read() {
                        n = n.saturating_add(1);
                        let mark3 = self.pos;
                        if delimiter {
                            if c == '=' {
                                delimiter = false;
                            } else {
                                sb.push(c);
                            }
                            continue;
                        }
                        if em.first() == Some(&c) {
                            if em.len() > 1 {
                                if self.read() == em.get(1).copied() {
                                    n = n.saturating_add(1);
                                    at_eof = false;
                                    break;
                                }
                                self.pos = mark3;
                                n = n.saturating_sub(1);
                            } else {
                                at_eof = false;
                                break;
                            }
                        }
                        sb.push(c);
                    }
                    let command = sb;
                    if command.is_empty() {
                        return Err(compile_error("Empty mustache", self.line));
                    }
                    let ch = command.chars().next().unwrap_or(' ');
                    // ALLOC: the template, its params and its output, which the request limits bound.
                    let variable = java_trim(command.get(ch.len_utf8()..).unwrap_or("")).to_owned();
                    match ch {
                        '#' | '^' | '<' | '?' | '$' => {
                            start_of_line = start_of_line && only_whitespace;
                            let line = self.line;
                            let body = self.compile(
                                Some(&variable),
                                sm.clone(),
                                em.clone(),
                                start_of_line,
                                depth.saturating_add(1),
                            )?;
                            let lines = self.line.saturating_sub(line);
                            if !only_whitespace || lines == 0 {
                                mv.write(&out, &sm, &em);
                            }
                            out.clear();
                            match ch {
                                '#' => iterable_code(&mut mv, &variable, body, line, &sm, &em)?,
                                '^' => {
                                    reject_dynamic(&variable)?;
                                    mv.push(
                                        Kind::Section {
                                            name: variable,
                                            body,
                                            inverted: true,
                                        },
                                        line,
                                        &sm,
                                        &em,
                                    );
                                }
                                '<' => mv.push(
                                    Kind::Extend {
                                        name: variable,
                                        body,
                                    },
                                    line,
                                    &sm,
                                    &em,
                                ),
                                // ALLOC: the template, its params and its output, which the request limits bound.
                                _ => return Err(unsupported(&format!("the [{ch}] tag"))),
                            }
                            iterable = lines != 0;
                        }
                        '/' => {
                            if !start_of_line || !only_whitespace {
                                mv.write(&out, &sm, &em);
                            }
                            if Some(variable.as_str()) != tag {
                                let tag_text = tag.unwrap_or("null");
                                return Err(compile_error(
                                    // ALLOC: the template, its params and its output, which the request limits bound.
                                    format!(
                                        "Mismatched start/end tags: {tag_text} != {variable} in {FILE}:{}",
                                        self.line
                                    ),
                                    self.line,
                                ));
                            }
                            return Ok(mv.0);
                        }
                        '>' => {
                            mv.write(&out, &sm, &em);
                            out.clear();
                            mv.push(Kind::Partial { name: variable }, self.line, &sm, &em);
                        }
                        '{' => {
                            mv.write(&out, &sm, &em);
                            out.clear();
                            let mut name = variable.clone();
                            if em.get(1) != Some(&'}') {
                                name.pop();
                            } else if self.read() != Some('}') {
                                n = n.saturating_add(1);
                                return Err(compile_error(
                                    // ALLOC: the template, its params and its output, which the request limits bound.
                                    format!(
                                        "Improperly closed variable: {variable} in {FILE}:{}@{n}",
                                        self.line
                                    ),
                                    self.line,
                                ));
                            }
                            reject_dynamic(&name)?;
                            mv.push(
                                Kind::Value {
                                    name,
                                    encoded: false,
                                },
                                self.line,
                                &sm,
                                &em,
                            );
                        }
                        '&' => {
                            mv.write(&out, &sm, &em);
                            out.clear();
                            reject_dynamic(&variable)?;
                            mv.push(
                                Kind::Value {
                                    name: variable,
                                    encoded: false,
                                },
                                self.line,
                                &sm,
                                &em,
                            );
                        }
                        '%' => return Err(unsupported("a pragma")),
                        '!' => {
                            mv.push(Kind::Comment { text: variable }, self.line, &sm, &em);
                            mv.write(&out, &sm, &em);
                            out.clear();
                        }
                        '=' => {
                            mv.write(&out, &sm, &em);
                            out.clear();
                            let trimmed = java_trim(command.get(1..).unwrap_or(""));
                            // ALLOC: the template, its params and its output, which the request limits bound.
                            let parts: Vec<&str> = trimmed.split_whitespace().collect();
                            let [open, close] = parts.as_slice() else {
                                return Err(compile_error(
                                    // ALLOC: the template, its params and its output, which the request limits bound.
                                    format!("Invalid delimiter string: {trimmed}"),
                                    self.line,
                                ));
                            };
                            // ALLOC: the template, its params and its output, which the request limits bound.
                            sm = open.chars().collect();
                            // ALLOC: the template, its params and its output, which the request limits bound.
                            em = close.chars().collect();
                        }
                        _ => {
                            if at_eof {
                                return Err(compile_error("Improperly closed variable", self.line));
                            }
                            mv.write(&out, &sm, &em);
                            out.clear();
                            // ALLOC: the template, its params and its output, which the request limits bound.
                            let name = java_trim(&command).to_owned();
                            reject_dynamic(&name)?;
                            mv.push(
                                Kind::Value {
                                    name,
                                    encoded: true,
                                },
                                self.line,
                                &sm,
                                &em,
                            );
                        }
                    }
                    start_of_line = false;
                    continue;
                }
                self.pos = mark;
            }
            only_whitespace = only_whitespace && matches!(c, ' ' | '\t' | '\r');
            out.push(c);
        }
        mv.write(&out, &sm, &em);
        if tag.is_none() {
            mv.push(Kind::Eof, self.line, &sm, &em);
            Ok(mv.0)
        } else {
            let tag_text = tag.unwrap_or_default();
            Err(compile_error(
                // ALLOC: the template, its params and its output, which the request limits bound.
                format!("Failed to close '{tag_text}' tag"),
                start_line,
            ))
        }
    }
}
