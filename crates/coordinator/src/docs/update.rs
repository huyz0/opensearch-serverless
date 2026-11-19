//! What an update does to the document it read: the options it accepts, how
//! often it retries, the plan its read gives, how a script's chosen operation
//! is understood and the condition its write carries.

/// `SequenceNumbers.UNASSIGNED_SEQ_NO`: no condition set.
pub const UNASSIGNED_SEQ_NO: i64 = -2;

/// An update that cannot go ahead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A request the caller must change: 400.
    BadRequest(&'static str),
    /// An option this node does not support: 501.
    NotImplemented(&'static str),
    /// The document changed since the caller's condition: 409.
    Conflict(String),
    /// The document does not exist and the request cannot create it: 404.
    Missing(String),
}

/// The options of an update that decide whether it is accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    /// `retry_on_conflict`.
    pub retry_on_conflict: i32,
    /// `scripted_upsert`.
    pub scripted_upsert: bool,
    /// Whether the request has a script.
    pub has_script: bool,
    /// `require_alias`.
    pub require_alias: bool,
    /// Whether `wait_for_active_shards` is above one.
    pub active_shards_above_one: bool,
    /// Whether the doc or upsert carry extra field values.
    pub extra_field_values: bool,
}

/// The document as the update's read found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Existing {
    /// Whether it exists.
    pub found: bool,
    /// Its sequence number.
    pub seq_no: i64,
    /// Its primary term.
    pub primary_term: i64,
}

/// What the request says of the update itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Request {
    /// `doc_as_upsert`.
    pub doc_as_upsert: bool,
    /// Whether the request has an upsert document.
    pub has_upsert: bool,
    /// Whether the request has a script.
    pub has_script: bool,
    /// `detect_noop`.
    pub detect_noop: bool,
    /// The caller's `if_seq_no`, or [`UNASSIGNED_SEQ_NO`].
    pub if_seq_no: i64,
    /// The caller's `if_primary_term`.
    pub if_primary_term: i64,
}

/// What the update does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The document does not exist: create it from the doc (`from_doc`) or
    /// from the upsert.
    Create {
        /// Whether the doc, rather than the upsert, is the source.
        from_doc: bool,
    },
    /// Run the script over the document.
    Script,
    /// Merge the doc into the document.
    Merge {
        /// Whether an unchanged merge is a noop.
        detect_noop: bool,
    },
}

/// What a script's `ctx.op` makes of the update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptOp {
    /// Write the edited source over the existing document.
    Index,
    /// Create the document from the edited source.
    Create,
    /// Delete the existing document.
    Delete,
    /// Do nothing.
    Noop,
}

/// The condition and creation requirement a write carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteCondition {
    /// The sequence number the write requires.
    pub if_seq_no: i64,
    /// The primary term the write requires.
    pub if_primary_term: i64,
    /// Whether the document must not exist.
    pub require_absent: bool,
}

/// Whether the update's options are accepted.
///
/// # Errors
///
/// The shell's refusal, checking in this order: retries, scripted upsert,
/// alias and active shards, extra field values.
pub fn validate(options: &Options) -> Result<(), Refused> {
    if options.retry_on_conflict < 0 {
        return Err(Refused::BadRequest(
            "retry_on_conflict must be non negative",
        ));
    }
    if options.scripted_upsert && !options.has_script {
        return Err(Refused::BadRequest(
            "scripted_upsert was given without a script",
        ));
    }
    if options.require_alias || options.active_shards_above_one {
        return Err(Refused::NotImplemented(
            "require_alias and active-shard counts above one are not supported on updates",
        ));
    }
    if options.extra_field_values {
        return Err(Refused::NotImplemented(
            "extra field values are not supported on updates",
        ));
    }
    Ok(())
}

/// How many times an update is tried: once, and again for each retry on a
/// version conflict.
#[must_use]
pub fn attempts(retry_on_conflict: i32) -> u64 {
    u64::from(retry_on_conflict.unsigned_abs()).saturating_add(1)
}

/// What the update does about the document it read.
///
/// # Errors
///
/// [`Refused::Conflict`] when the caller's condition no longer holds for a
/// found document, and [`Refused::Missing`] when the document is missing and
/// the request has neither a doc to upsert nor an upsert.
pub fn plan(
    index: &str,
    id: &str,
    existing: &Existing,
    request: &Request,
) -> Result<Plan, Refused> {
    if existing.found
        && request.if_seq_no != UNASSIGNED_SEQ_NO
        && (existing.seq_no != request.if_seq_no
            || existing.primary_term != request.if_primary_term)
    {
        // The condition is checked on the read, before a script or a noop
        // could slip past the write path.
        // ALLOC: one message.
        return Err(Refused::Conflict(format!(
            "[{id}]: version conflict, required seqNo [{}], primary term [{}]. \
             current document has seqNo [{}] and primary term [{}]",
            request.if_seq_no, request.if_primary_term, existing.seq_no, existing.primary_term
        )));
    }
    if !existing.found {
        return if request.doc_as_upsert {
            Ok(Plan::Create { from_doc: true })
        } else if request.has_upsert {
            Ok(Plan::Create { from_doc: false })
        } else {
            // ALLOC: one message.
            Err(Refused::Missing(format!(
                "document missing: [{index}]/[{id}]"
            )))
        };
    }
    if request.has_script {
        Ok(Plan::Script)
    } else {
        Ok(Plan::Merge {
            detect_noop: request.detect_noop,
        })
    }
}

/// What a script's `ctx.op` makes of the update: an existing document may be
/// indexed or deleted, a missing one only created, and anything else,
/// including unknown values, is a noop.
#[must_use]
pub fn script_operation(found: bool, op: &str) -> ScriptOp {
    match (found, op) {
        (true, "index") => ScriptOp::Index,
        (true, "delete") => ScriptOp::Delete,
        (false, "create") => ScriptOp::Create,
        _ => ScriptOp::Noop,
    }
}

/// The condition the update's write carries: the caller's own when they set
/// one, else the identity just read for a found document (so two concurrent
/// partial updates cannot both win), else a requirement that the document
/// not exist (so two concurrent upserts cannot both create).
#[must_use]
pub fn write_condition(
    existing: &Existing,
    if_seq_no: i64,
    if_primary_term: i64,
) -> WriteCondition {
    if if_seq_no != UNASSIGNED_SEQ_NO {
        return WriteCondition {
            if_seq_no,
            if_primary_term,
            require_absent: false,
        };
    }
    if existing.found {
        WriteCondition {
            if_seq_no: existing.seq_no,
            if_primary_term: existing.primary_term,
            require_absent: false,
        }
    } else {
        WriteCondition {
            if_seq_no,
            if_primary_term,
            require_absent: true,
        }
    }
}
