//! An alias record: a name that stands for some indices, in the same
//! register namespace as index descriptors.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/cluster/AliasRecord.java`
//! (`toBytes`, `fromStream`, the constructor's write-member rule and the
//! accessors resolution needs). A record is an alias, or a data stream: an
//! alias with a generation whose last index takes its writes. While an alias
//! mutation prepares its members' hints, the record also carries the
//! mutation's deadline and the members it adds.
//!
//! Fields are written in `toBytes`'s order, each optional group only when
//! the Java writes it, so the bytes are the Java's.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt};
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "alias record";

/// The root field that makes a register an alias (`AliasRecord.DISCRIMINATOR`).
pub const ALIAS_DISCRIMINATOR: &str = "alias";

/// A mutation reserving an alias while it prepares its new members' hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasMutation<'a> {
    /// When another writer may recover the mutation, if it is abandoned: a
    /// time after the epoch, above zero.
    pub until: i64,
    /// Whether no alias has been published yet, so the record is not one.
    pub creating: bool,
    /// The members whose hints the mutation is preparing.
    pub additions: Vec<Cow<'a, str>>,
}

/// An alias or data stream record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasRecord<'a> {
    /// The alias's name.
    pub name: Cow<'a, str>,
    /// The indices it stands for, in order: a data stream's oldest first.
    /// Never empty.
    pub indices: Vec<Cow<'a, str>>,
    /// Whether this is a data stream.
    pub data_stream: bool,
    /// A data stream's generation, the number its next backing index takes;
    /// written only for a data stream.
    pub generation: i64,
    /// A data stream's timestamp field; written only for a data stream.
    pub timestamp_field: Option<Cow<'a, str>>,
    /// The member a plain alias marks as its write index. Kept only when it
    /// is a member of a plain alias, as the Java's constructor keeps it.
    pub write_member: Option<Cow<'a, str>>,
    /// The mutation in progress, if any.
    pub mutation: Option<AliasMutation<'a>>,
}

const FIELDS: [&str; 7] = [
    ALIAS_DISCRIMINATOR,
    "indices",
    "data_stream",
    "generation",
    "timestamp_field",
    "write_index",
    "mutation",
];

impl<'a> AliasRecord<'a> {
    /// A plain alias of `indices`.
    pub fn new(name: &'a str, indices: &[&'a str]) -> Self {
        Self {
            name: Cow::Borrowed(name),
            // ALLOC: one entry a member, the caller's own.
            indices: indices.iter().map(|i| Cow::Borrowed(*i)).collect(),
            data_stream: false,
            generation: 0,
            timestamp_field: None,
            write_member: None,
            mutation: None,
        }
    }

    /// The write member as the Java's constructor keeps it: none for a data
    /// stream, or for a name that is not a member.
    pub fn kept_write_member(&self) -> Option<&str> {
        let member = self.write_member.as_deref()?;
        (!self.data_stream && self.indices.iter().any(|i| i == member)).then_some(member)
    }

    /// The index this name marks as its write index: a data stream's newest
    /// backing index, or the member a plain alias marks.
    pub fn write_index(&self) -> Option<&str> {
        if self.data_stream {
            return self.indices.last().map(AsRef::as_ref);
        }
        self.kept_write_member()
    }

    /// The index a write through this name goes to, as core sends it: the
    /// write index, or the only index of an alias that marks none. `None`
    /// when it names several and marks none, so a write is refused.
    pub fn writes_to(&self) -> Option<&str> {
        self.write_index().or(match self.indices.as_slice() {
            [only] => Some(only.as_ref()),
            _ => None,
        })
    }

    /// Whether a mutation is preparing hints before publishing membership.
    pub fn mutation_pending(&self) -> bool {
        self.mutation.is_some()
    }

    /// Whether the pending mutation is a creation not yet published: such a
    /// record resolves as no alias.
    pub fn creation_pending(&self) -> bool {
        self.mutation.as_ref().is_some_and(|m| m.creating)
    }

    /// Whether `index`'s hint naming this alias must stay: it is a member,
    /// or one the pending mutation adds.
    pub fn protects_hint(&self, index: &str) -> bool {
        self.indices.iter().any(|i| i == index)
            || self
                .mutation
                .as_ref()
                .is_some_and(|m| m.additions.iter().any(|a| a == index))
    }

    /// Writes the record as the Java's `toBytes`, into the front of `buf`,
    /// and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a record the Java would refuse to read
    /// back: no indices, or a mutation whose deadline is not above zero or
    /// that adds an empty name; [`FormatError::TooLarge`] when `buf` is
    /// shorter than the record.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        self.check()?;
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field(ALIAS_DISCRIMINATOR, &self.name);
        out.start_array_field("indices");
        for index in &self.indices {
            out.string(index);
        }
        out.end_array();
        if self.data_stream {
            out.bool_field("data_stream", true);
            out.int_field("generation", self.generation);
            match &self.timestamp_field {
                Some(field) => out.string_field("timestamp_field", field),
                None => out.null_field("timestamp_field"),
            }
        }
        if let Some(member) = self.kept_write_member() {
            out.string_field("write_index", member);
        }
        if let Some(mutation) = &self.mutation {
            out.start_object_field("mutation");
            out.int_field("until", mutation.until);
            out.bool_field("creating", mutation.creating);
            out.start_array_field("additions");
            for added in &mutation.additions {
                out.string(added);
            }
            out.end_array();
            out.end_object();
        }
        out.end_object();
        out.finish(WHAT)
    }

    /// The Java's `fromStream` checks.
    fn check(&self) -> Result<(), FormatError> {
        if self.indices.is_empty() {
            return Err(corrupt(WHAT, "an alias stands for at least one index"));
        }
        if let Some(mutation) = &self.mutation
            && (mutation.until <= 0 || mutation.additions.iter().any(|a| a.is_empty()))
        {
            return Err(corrupt(WHAT, "invalid alias mutation"));
        }
        Ok(())
    }

    /// Parses a record as the Java's `fromStream`: the name and at least one
    /// index required, unknown fields skipped, a write member that is not a
    /// member of a plain alias dropped, and a mutation's fields checked. A
    /// repeated known field is refused, as Jackson refuses any; a repeated
    /// unknown one is not checked.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a repeated field, a
    /// record the Java refuses, or a value of a JSON type the Java never
    /// writes there, which it would coerce: a name, member or timestamp
    /// field that is not a string, a flag that is not a boolean, a
    /// generation that is not an integer.
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        let mut json = Reader::new(value, WHAT);
        json.start_object("an alias record is an object")?;
        let mut name = None;
        let mut record = Self {
            name: Cow::Borrowed(""),
            // ALLOC: none until a member is pushed.
            indices: Vec::new(),
            data_stream: false,
            generation: 0,
            timestamp_field: None,
            write_member: None,
            mutation: None,
        };
        let mut seen = 0u8;
        while let Some(field) = json.next_field()? {
            let Some(index) = FIELDS.iter().position(|known| *known == field) else {
                json.skip_value()?;
                continue;
            };
            let bit = 1u8 << index;
            if seen & bit != 0 {
                return Err(json.error("a field is repeated"));
            }
            seen |= bit;
            match field.as_ref() {
                "indices" => record.indices = strings(&mut json, "indices hold strings")?,
                "mutation" => record.mutation = Some(mutation(&mut json)?),
                known => match (known, json.expect_token()?) {
                    (ALIAS_DISCRIMINATOR, Token::Str(text)) => name = Some(text),
                    ("data_stream", Token::Bool(flag)) => record.data_stream = flag,
                    ("generation", Token::Int(n)) => record.generation = n,
                    ("timestamp_field", Token::Str(text)) => record.timestamp_field = Some(text),
                    ("timestamp_field", Token::Null) => record.timestamp_field = None,
                    ("write_index", Token::Str(text)) => record.write_member = Some(text),
                    _ => {
                        return Err(json.error(
                            "names are strings, data_stream a boolean, generation an integer",
                        ));
                    }
                },
            }
        }
        json.finish()?;
        let Some(name) = name else {
            return Err(corrupt(WHAT, "not an alias record: no alias field"));
        };
        record.name = name;
        if record.kept_write_member().is_none() {
            record.write_member = None;
        }
        record.check()?;
        Ok(record)
    }
}

/// A mutation object: `until` a positive integer, `creating` a boolean and
/// `additions` non-empty strings, all required; other fields skipped.
fn mutation<'a>(json: &mut Reader<'a>) -> Result<AliasMutation<'a>, FormatError> {
    json.start_object("invalid alias mutation")?;
    let (mut until, mut creating, mut additions) = (None, None, None);
    while let Some(field) = json.next_field()? {
        match field.as_ref() {
            "until" if until.is_none() => {
                until = match json.expect_token()? {
                    // Above zero, as `check` requires.
                    Token::Int(n) => Some(n),
                    _ => return Err(json.error("invalid alias mutation fields")),
                };
            }
            "creating" if creating.is_none() => {
                creating = match json.expect_token()? {
                    Token::Bool(flag) => Some(flag),
                    _ => return Err(json.error("invalid alias mutation fields")),
                };
            }
            "additions" if additions.is_none() => {
                additions = Some(strings(json, "invalid alias mutation member")?);
            }
            "until" | "creating" | "additions" => {
                return Err(json.error("a field is repeated"));
            }
            _ => json.skip_value()?,
        }
    }
    let (Some(until), Some(creating), Some(additions)) = (until, creating, additions) else {
        return Err(json.error("invalid alias mutation fields"));
    };
    Ok(AliasMutation {
        until,
        creating,
        additions,
    })
}

/// An array of strings.
fn strings<'a>(
    json: &mut Reader<'a>,
    reason: &'static str,
) -> Result<Vec<Cow<'a, str>>, FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error(reason));
    }
    // ALLOC: one entry a name, bounded by the register's length.
    let mut names = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(names),
            Token::Str(name) => names.push(name),
            _ => return Err(json.error(reason)),
        }
    }
}
