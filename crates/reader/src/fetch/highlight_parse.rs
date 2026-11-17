//! The search body's `highlight`: `HighlightBuilder.fromXContent` at
//! OpenSearch 3.10.0 for the options of the unified highlighter that this
//! reader serves. Each field's options are its own, else the highlight's,
//! else the defaults of `SearchHighlightContext.FieldOptions`. A key of
//! OpenSearch's that is not served yet is refused; an unknown key fails as
//! it does there.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, int_value, suggest};
use oss_rest::json::Location;

/// The options of one highlighted field, merged over the highlight's and the
/// defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The tag before a match: the first of `pre_tags`.
    pub pre_tag: String,
    /// The tag after a match: the first of `post_tags`.
    pub post_tag: String,
    /// `fragment_size`: the most characters of a fragment, `0` for none.
    pub fragment_size: i32,
    /// `number_of_fragments`: how many fragments, `0` for the whole value.
    pub number_of_fragments: i32,
    /// `no_match_size`: the characters shown when nothing matches.
    pub no_match_size: i32,
    /// `order: score`: fragments by score rather than by position.
    pub score_ordered: bool,
    /// `encoder: html`: the text is escaped.
    pub html: bool,
    /// `require_field_match`: only the field's own matches are marked.
    pub require_field_match: bool,
    /// `boundary_scanner: word`: fragments end at words, not sentences.
    pub word_boundary: bool,
}

/// A parsed `highlight`: the fields to highlight, in request order, each
/// with its merged options.
#[derive(Clone, Debug, PartialEq)]
pub struct HighlightSpec {
    /// The field names or `*` patterns.
    pub fields: Vec<(String, Options)>,
}

/// What one level (the highlight or a field) sets.
#[derive(Clone, Default)]
struct Partial {
    pre: Option<String>,
    post: Option<String>,
    fragment_size: Option<i32>,
    number_of_fragments: Option<i32>,
    no_match_size: Option<i32>,
    score_ordered: Option<bool>,
    html: Option<bool>,
    require_field_match: Option<bool>,
    word_boundary: Option<bool>,
}

impl Partial {
    /// This level's options over `outer`'s, over the defaults.
    fn merged(&self, outer: &Partial) -> Options {
        Options {
            // ALLOC: one tag a field, at most the body's recording.
            pre_tag: self
                .pre
                .clone()
                .or_else(|| outer.pre.clone())
                // ALLOC: one default tag a field, as above.
                .unwrap_or_else(|| "<em>".to_owned()),
            // ALLOC: as above.
            post_tag: self
                .post
                .clone()
                .or_else(|| outer.post.clone())
                // ALLOC: as above.
                .unwrap_or_else(|| "</em>".to_owned()),
            fragment_size: self.fragment_size.or(outer.fragment_size).unwrap_or(100),
            number_of_fragments: self
                .number_of_fragments
                .or(outer.number_of_fragments)
                .unwrap_or(5),
            no_match_size: self.no_match_size.or(outer.no_match_size).unwrap_or(0),
            score_ordered: self.score_ordered.or(outer.score_ordered).unwrap_or(false),
            html: self.html.or(outer.html).unwrap_or(false),
            require_field_match: self
                .require_field_match
                .or(outer.require_field_match)
                .unwrap_or(true),
            word_boundary: self.word_boundary.or(outer.word_boundary).unwrap_or(false),
        }
    }
}

/// Keys OpenSearch reads that this reader does not serve yet.
const UNSUPPORTED: &[&str] = &[
    "fragmenter",
    "force_source",
    "highlight_query",
    "highlight_filter",
    "boundary_chars",
    "boundary_max_scan",
    "boundary_scanner_locale",
    "tags_schema",
    "matched_fields",
    "max_analyzer_offset",
    "phrase_limit",
    "options",
    "fragment_offset",
    "highlight_query",
];

/// Which object a key is read in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Highlight,
    Field,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Highlight => "highlight",
            Level::Field => "highlight_field",
        }
    }

    fn keys(self) -> &'static [&'static str] {
        match self {
            Level::Highlight => &[
                "fields",
                "pre_tags",
                "post_tags",
                "fragment_size",
                "number_of_fragments",
                "no_match_size",
                "order",
                "encoder",
                "require_field_match",
                "type",
                "boundary_scanner",
            ],
            Level::Field => &[
                "pre_tags",
                "post_tags",
                "fragment_size",
                "number_of_fragments",
                "no_match_size",
                "order",
                "encoder",
                "require_field_match",
                "type",
                "boundary_scanner",
            ],
        }
    }
}

fn unsupported(what: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("the highlight's {what}"))
}

/// How reading a key fails: as OpenSearch's parser fails, which its nested
/// parsers wrap, or as a refusal of what is not served yet.
enum Fail {
    Parse(DslError),
    Refuse(DslError),
}

impl Fail {
    fn into_error(self) -> DslError {
        match self {
            Fail::Parse(e) | Fail::Refuse(e) => e,
        }
    }
}

/// The `highlight` object, the parser on its start.
pub fn parse_highlight(p: &mut Parser<'_>) -> Result<HighlightSpec, DslError> {
    // ALLOC: one entry a field, bounded by the body's recording.
    let mut fields: Vec<(String, Partial)> = Vec::new();
    let global = read_level(p, Level::Highlight, &mut fields).map_err(Fail::into_error)?;
    Ok(HighlightSpec {
        // ALLOC: one entry a field, bounded by the body's recording.
        fields: fields
            .into_iter()
            .map(|(name, own)| (name, own.merged(&global)))
            // ALLOC: as above.
            .collect(),
    })
}

/// One level's keys, the parser on its start object; `fields` collects the
/// highlight's fields.
fn read_level(
    p: &mut Parser<'_>,
    level: Level,
    fields: &mut Vec<(String, Partial)>,
) -> Result<Partial, Fail> {
    let mut set = Partial::default();
    let mut name = "";
    let mut name_at = p.location();
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                name = p.text();
                name_at = p.location();
            }
            Some(Kind::StartObject) if name == "fields" && level == Level::Highlight => {
                read_fields(p, fields)?;
            }
            Some(Kind::StartArray) if name == "pre_tags" || name == "post_tags" => {
                let tag = first_tag(p)?;
                if name == "pre_tags" {
                    set.pre = Some(tag);
                } else {
                    set.post = Some(tag);
                }
            }
            Some(Kind::StartObject | Kind::StartArray) => {
                return Err(
                    if level.keys().contains(&name) || UNSUPPORTED.contains(&name) {
                        // ALLOC: error path; one message.
                        Fail::Refuse(unsupported(&format!("[{name}] of this kind")))
                    } else {
                        Fail::Parse(unknown(name_at, level, name))
                    },
                );
            }
            Some(k) => scalar(p, level, name, name_at, k, &mut set)?,
        }
    }
    if set.pre.is_some() != set.post.is_some() {
        return Err(Fail::Refuse(unsupported(
            "[pre_tags] without [post_tags], or the reverse",
        )));
    }
    Ok(set)
}

/// The first of a `pre_tags` or `post_tags` array of strings, the parser on
/// its start; the unified highlighter uses no other.
fn first_tag(p: &mut Parser<'_>) -> Result<String, Fail> {
    let mut first = None;
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => break,
            Some(Kind::String) => {
                if first.is_none() {
                    // ALLOC: one tag, bounded by the body's recording.
                    first = Some(p.text().to_owned());
                }
            }
            Some(_) => return Err(Fail::Refuse(unsupported("[pre_tags] entry of this kind"))),
        }
    }
    first.ok_or_else(|| Fail::Refuse(unsupported("an empty [pre_tags]")))
}

fn unknown(at: Location, level: Level, name: &str) -> DslError {
    DslError::xcontent(
        at,
        // ALLOC: error path; one message.
        &format!(
            "[{}] unknown field [{name}]{}",
            level.name(),
            suggest(name, level.keys())
        ),
        None,
    )
}

fn scalar(
    p: &mut Parser<'_>,
    level: Level,
    name: &str,
    name_at: Location,
    kind: Kind,
    set: &mut Partial,
) -> Result<(), Fail> {
    if !level.keys().contains(&name) {
        if UNSUPPORTED.contains(&name) {
            // ALLOC: error path; one message.
            return Err(Fail::Refuse(unsupported(&format!("[{name}]"))));
        }
        return Err(Fail::Parse(unknown(name_at, level, name)));
    }
    let at = p.location();
    let refused = || {
        // ALLOC: error path; one message.
        Fail::Refuse(unsupported(&format!("[{name}] of this kind")))
    };
    let int = |p: &Parser<'_>| -> Result<i32, Fail> {
        let n = int_value(p).map_err(|e| {
            Fail::Parse(DslError::xcontent(
                at,
                // ALLOC: error path; one message.
                &format!("[{}] failed to parse field [{name}]", level.name()),
                Some(e),
            ))
        })?;
        if n < 0 {
            return Err(refused());
        }
        Ok(n)
    };
    if kind == Kind::Null {
        return Err(refused());
    }
    match name {
        "fragment_size" => set.fragment_size = Some(int(p)?),
        "number_of_fragments" => set.number_of_fragments = Some(int(p)?),
        "no_match_size" => set.no_match_size = Some(int(p)?),
        "order" => match p.text() {
            "score" => set.score_ordered = Some(true),
            "none" => set.score_ordered = Some(false),
            _ => return Err(refused()),
        },
        "encoder" => match p.text() {
            "html" => set.html = Some(true),
            "default" => set.html = Some(false),
            _ => return Err(refused()),
        },
        "require_field_match" => {
            set.require_field_match = Some(bool_value(p).map_err(Fail::Parse)?)
        }
        "type" if p.text() == "unified" => {}
        "boundary_scanner" => match p.text() {
            "sentence" => set.word_boundary = Some(false),
            "word" => set.word_boundary = Some(true),
            _ => return Err(refused()),
        },
        _ => return Err(refused()),
    }
    Ok(())
}

/// The `fields` object: each key a field name or pattern, each value that
/// field's options. A parse error in a field is wrapped as the nested
/// parsers of OpenSearch wrap it.
fn read_fields(p: &mut Parser<'_>, fields: &mut Vec<(String, Partial)>) -> Result<(), Fail> {
    let mut name = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => return Ok(()),
            Some(Kind::FieldName) => name = p.text(),
            Some(Kind::StartObject) => {
                // ALLOC: empty; a field level has no fields.
                let own = match read_level(p, Level::Field, &mut Vec::new()) {
                    Ok(own) => own,
                    Err(Fail::Parse(inner)) => {
                        let at = p.location();
                        let titled = DslError::xcontent(
                            at,
                            // ALLOC: error path; one message.
                            &format!("[fields] failed to parse field [{name}]"),
                            Some(inner),
                        );
                        return Err(Fail::Parse(DslError::xcontent(
                            at,
                            "[highlight] failed to parse field [fields]",
                            Some(titled),
                        )));
                    }
                    Err(refused) => return Err(refused),
                };
                // ALLOC: one entry a field, bounded by the body's recording.
                fields.push((name.to_owned(), own));
            }
            Some(_) => return Err(Fail::Refuse(unsupported("[fields] entry of this kind"))),
        }
    }
}

#[cfg(test)]
#[path = "highlight_parse_tests.rs"]
mod tests;
