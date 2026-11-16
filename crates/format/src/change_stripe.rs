//! The value of a change stripe: the recent changes to the descriptors of
//! the index names that hash to it.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ChangeStripes.java`
//! (`State`, `stripeOf`). A stripe is text: a head line `v1 <epoch>
//! <generation>`, then one line an entry, `<generation> D <name>` for an
//! applied change or `<generation> P <name>` for a delete about to be
//! applied. Every bump increments the generation; the entries kept are the
//! last [`KEPT`] applied ones and every unfinished delete, and past
//! [`KEPT_PENDING`] of those the epoch moves on and the entries are dropped.

use crate::error::{FormatError, corrupt};
use crate::json::out::Out;

const WHAT: &str = "change stripe";

/// How many stripes there are. Fixed: every node of a cluster hashes a name
/// to the same one.
pub const STRIPES: usize = 16;

/// How many applied changes a stripe keeps.
pub const KEPT: usize = 256;

/// How many unfinished deletes a stripe keeps before it starts a new epoch.
pub const KEPT_PENDING: usize = 64;

/// The longest name an entry holds, in UTF-8 bytes: an index name's limit.
pub const MAX_ENTRY_NAME_BYTES: usize = 255;

/// The longest stripe: a head of a version and two `i64`s, and every entry
/// kept at its longest (an `i64`, a kind and a 255-byte name).
pub const MAX_CHANGE_STRIPE_BYTES: usize =
    HEAD_BYTES + (KEPT + KEPT_PENDING) * (20 + 3 + MAX_ENTRY_NAME_BYTES + 1);

// `v1 `, two numbers of at most 20 characters, a space and a newline.
const HEAD_BYTES: usize = 3 + 20 + 1 + 20 + 1;

/// What a bump records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// A delete about to be applied.
    PendingDelete,
    /// A delete attempt that has finished, applied or refused: it finishes
    /// the pending entry before it.
    Deleted,
    /// Any other applied change: a mapping or settings update.
    Changed,
}

/// One change a stripe keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripeEntry<'a> {
    /// The stripe's generation the bump made.
    pub generation: i64,
    /// A delete about to be applied, rather than an applied change.
    pub pending: bool,
    /// The index.
    pub name: &'a str,
}

/// A stripe's value, parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeStripe<'a> {
    /// Moves on when the entries are dropped wholesale.
    pub epoch: i64,
    /// Incremented by every bump.
    pub generation: i64,
    /// The changes kept, oldest first.
    pub entries: Vec<StripeEntry<'a>>,
}

/// The stripe an index name hashes to: the Java's
/// `Math.floorMod(name.hashCode(), STRIPES)`, over the name's UTF-16 units.
pub fn stripe_of(name: &str) -> usize {
    let hash = name.encode_utf16().fold(0i32, |h, unit| {
        h.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    usize::try_from(hash.rem_euclid(16)).unwrap_or(0)
}

impl<'a> ChangeStripe<'a> {
    /// Parses a stripe as the Java's `State.parse`: lines split on `\n`
    /// with blank ones skipped, the head split on spaces, each entry on its
    /// first two, the name taking the rest.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for bytes that are not UTF-8, a head that
    /// is not `v1` and two integers, or an entry without a generation, a
    /// kind and a name.
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        let text = std::str::from_utf8(value).map_err(|_| corrupt(WHAT, "not UTF-8"))?;
        let mut lines = text.split('\n');
        let head = lines.next().unwrap_or_default();
        // Java's split drops trailing empty parts: `v1 1 2 ` is a head.
        let mut parts = head.trim_end_matches(' ').split(' ');
        let (Some("v1"), Some(epoch), Some(generation), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(corrupt(WHAT, "not a change stripe"));
        };
        let mut stripe = Self {
            epoch: number(epoch)?,
            generation: number(generation)?,
            // ALLOC: none until an entry is pushed below.
            entries: Vec::new(),
        };
        for line in lines.filter(|line| !line.is_empty()) {
            let mut parts = line.splitn(3, ' ');
            let (Some(generation), Some(kind), Some(name)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Err(corrupt(WHAT, "an entry is a generation, a kind and a name"));
            };
            // ALLOC: one entry a line, at most KEPT + KEPT_PENDING kept.
            stripe.entries.push(StripeEntry {
                generation: number(generation)?,
                // As the Java's: anything but `P` is applied.
                pending: kind == "P",
                name,
            });
        }
        Ok(stripe)
    }

    /// Writes the stripe as the Java's `State.encode`, into the front of
    /// `buf`, and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is shorter than the stripe;
    /// [`MAX_CHANGE_STRIPE_BYTES`] suffices for any stripe of names of at
    /// most [`MAX_ENTRY_NAME_BYTES`].
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.raw(b"v1 ");
        out.int(self.epoch);
        out.raw(b" ");
        out.int(self.generation);
        out.raw(b"\n");
        for entry in &self.entries {
            out.int(entry.generation);
            out.raw(if entry.pending { b" P " } else { b" D " });
            out.raw(entry.name.as_bytes());
            out.raw(b"\n");
        }
        out.finish(WHAT)
    }

    /// Records a change to `name`, as the Java's `State.add`: the
    /// generation moves on and an entry is added; a finished delete drops
    /// the name's pending entries; past [`KEPT_PENDING`] pending entries
    /// the epoch moves on and every entry is dropped, and otherwise only
    /// the newest [`KEPT`] applied entries are kept.
    pub fn add(&mut self, name: &'a str, kind: ChangeKind) {
        self.generation = self.generation.saturating_add(1);
        if kind == ChangeKind::Deleted {
            // Only a delete finishes one: an update landing between a
            // delete's two bumps says nothing about whether it applied.
            self.entries.retain(|e| !(e.pending && e.name == name));
        }
        // ALLOC: one entry, the stripe trimmed back below.
        self.entries.push(StripeEntry {
            generation: self.generation,
            pending: kind == ChangeKind::PendingDelete,
            name,
        });
        let pending = self.entries.iter().filter(|e| e.pending).count();
        if pending > KEPT_PENDING {
            // Too many unfinished deletes to keep: a new epoch makes every
            // reader read its descriptors again.
            self.epoch = self.epoch.saturating_add(1);
            self.entries.clear();
            return;
        }
        let mut drop = self
            .entries
            .len()
            .saturating_sub(pending)
            .saturating_sub(KEPT);
        self.entries.retain(|e| {
            if drop > 0 && !e.pending {
                drop = drop.saturating_sub(1);
                return false;
            }
            true
        });
    }
}

/// An `i64` as Java's `Long.parseLong` reads one: an optional sign, then
/// decimal digits.
fn number(text: &str) -> Result<i64, FormatError> {
    text.parse()
        .map_err(|_| corrupt(WHAT, "a generation or epoch is not an integer"))
}
