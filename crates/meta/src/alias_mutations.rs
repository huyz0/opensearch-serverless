//! Changing aliases so a crashed writer never leaves an index's
//! `aliased_by` hint missing for an alias that names it.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/AliasMutations.java`.
//! A mutation runs in three steps, each a compare-and-swap or conditional
//! create:
//!
//! 1. **Reserve:** the alias register is written with a pending mutation
//!    that names the members it adds and a deadline.
//! 2. **Prepare:** every member it adds gains the alias in its
//!    descriptor's `aliased_by` hint, each swap checking that the
//!    reservation still holds. Members already in the alias keep theirs,
//!    so a change costs requests per member it adds or removes, never per
//!    member it keeps; the Java reads every member.
//! 3. **Publish:** the alias register is swapped to the new membership.
//!    Hints of members it dropped are then removed.
//!
//! A writer that finds a reservation past its deadline aborts it: it
//! restores the published record, or deletes a creation that never
//! published, and removes the hints the mutation added. While a mutation
//! is pending, nothing removes the hint of a member it adds, so a published
//! alias's members always carry its hint. The hint is a superset: a reader
//! confirms each name it lists ([`AliasMutations::aliases_of`]).
//!
//! The Java also marks the alias in the search digest rollups; those are
//! not ported yet.

use std::borrow::Cow;
use std::time::Duration;

use oss_format::AliasRecord;

use crate::descriptors::{DescriptorStore, Target};
use crate::error::MetaError;

/// Attempts at each step that races other writers, as the Java's.
const ATTEMPTS: usize = 4;

/// How long [`AliasMutations::resolve_for_mutation`] waits for another
/// writer's pending mutation, in total, as the Java's 250 ms.
const WAIT_MILLIS: u64 = 250;

/// The longest single pause in that wait.
const MAX_PAUSE_MILLIS: u64 = 64;

/// The time, and how a writer waits, for [`AliasMutations`].
pub struct AliasClock<'a> {
    /// Milliseconds since the epoch: deadlines are stamped and compared in
    /// it.
    pub now_millis: &'a dyn Fn() -> i64,
    /// Blocks the caller for a pause while another writer's mutation is
    /// pending.
    pub pause: &'a dyn Fn(Duration),
}

/// An alias as read for a mutation, with its register's generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasAt {
    /// The record.
    pub alias: AliasRecord<'static>,
    /// The generation to swap from.
    pub generation: u64,
}

/// What a name's register held, read for a mutation.
struct Read {
    generation: u64,
    alias: Option<AliasRecord<'static>>,
}

/// Alias mutations over one descriptor store, with the buffers they read
/// and write registers through.
pub struct AliasMutations<'a> {
    descriptors: &'a DescriptorStore,
    clock: AliasClock<'a>,
    lease_ttl_millis: i64,
    index_buf: Vec<u8>,
    alias_buf: Vec<u8>,
    write_buf: Vec<u8>,
}

impl<'a> AliasMutations<'a> {
    /// Mutations over `descriptors`, a reservation lasting
    /// `lease_ttl_millis`, through registers of at most `register_bytes`:
    /// a longer register is refused as too large. Keep one a writer; its
    /// buffers are taken once.
    pub fn new(
        descriptors: &'a DescriptorStore,
        clock: AliasClock<'a>,
        lease_ttl_millis: i64,
        register_bytes: usize,
    ) -> Self {
        Self {
            descriptors,
            clock,
            lease_ttl_millis,
            // ALLOC: three register buffers a writer, once, `register_bytes` each.
            index_buf: vec![0; register_bytes],
            alias_buf: vec![0; register_bytes],
            write_buf: vec![0; register_bytes],
        }
    }

    fn now(&self) -> i64 {
        (self.clock.now_millis)()
    }

    /// When a reservation made now may be recovered by another writer.
    fn deadline(&self) -> i64 {
        self.now().max(1).saturating_add(self.lease_ttl_millis)
    }

    /// `name`'s register, the alias it holds copied out.
    fn read(&mut self, name: &str) -> Result<Read, MetaError> {
        let resolved = self.descriptors.resolve_raw(name, &mut self.alias_buf)?;
        Ok(Read {
            generation: resolved.generation,
            alias: match resolved.target {
                Target::Alias(alias) => Some(alias.into_owned()),
                Target::Index(_) | Target::Absent => None,
            },
        })
    }

    /// Creates `alias` and adds it to each member's hint: the reservation's
    /// create, a read and swap each member, and the publishing swap. The
    /// generation it was published at.
    ///
    /// # Errors
    ///
    /// [`MetaError::AlreadyExists`] when the name is taken;
    /// [`MetaError::MissingMember`] for a member that is no index, after
    /// which the reservation is undone; [`MetaError::Superseded`] when
    /// another writer recovered the reservation first.
    pub fn create_alias(&mut self, alias: &AliasRecord<'_>) -> Result<u64, MetaError> {
        self.recover(&alias.name)?;
        let alias = alias.clone().into_owned();
        let pending = alias
            .preparing(alias.indices.clone(), self.deadline(), true)
            .ok_or(MetaError::InvalidArgument(
                "an alias to create must have no mutation pending",
            ))?;
        let reserved = self
            .descriptors
            .create_alias(&pending, self.now(), &mut self.write_buf)?;
        self.publish(&pending, &alias, reserved)?
            .ok_or(MetaError::Superseded)
    }

    /// Replaces `before`, read at `expected`, with `alias`: reserve, prepare
    /// the members it adds, publish, then drop the hints of members it
    /// removes. `None` when another writer got there first; a `before`
    /// with a mutation pending is recovered instead, and answers `None`.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidArgument`] when `before` and `alias` name
    /// different aliases, before any request; [`MetaError::MissingMember`]
    /// for an added member that is no index, after which the reservation is
    /// undone; [`MetaError::Superseded`] when the reservation was recovered
    /// while hints were prepared.
    pub fn update_alias(
        &mut self,
        before: &AliasRecord<'_>,
        alias: &AliasRecord<'_>,
        expected: u64,
    ) -> Result<Option<u64>, MetaError> {
        if before.name != alias.name {
            return Err(MetaError::InvalidArgument(
                "an alias update keeps the alias's name",
            ));
        }
        if before.mutation_pending() {
            self.recover(&alias.name)?;
            return Ok(None);
        }
        let alias = alias.clone().into_owned();
        let additions = alias
            .indices
            .iter()
            .filter(|i| !before.indices.contains(i))
            .cloned()
            // ALLOC: one entry an added member, bounded by the record.
            .collect();
        let pending = before
            .clone()
            .into_owned()
            .preparing(additions, self.deadline(), false)
            .ok_or(MetaError::InvalidArgument(
                "a published alias cannot be reserved",
            ))?;
        let Some(reserved) =
            self.descriptors
                .update_alias(&pending, expected, &mut self.write_buf)?
        else {
            return Ok(None);
        };
        self.publish(&pending, &alias, reserved)
    }

    /// Deletes `alias`, read at `expected`, if it has not changed, then
    /// removes its members' hints. `false` when it changed first, or had a
    /// mutation pending, which is recovered.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the store fails.
    pub fn delete_alias(
        &mut self,
        alias: &AliasRecord<'_>,
        expected: u64,
    ) -> Result<bool, MetaError> {
        if alias.mutation_pending() {
            self.recover(&alias.name)?;
            return Ok(false);
        }
        let deleted = self
            .descriptors
            .delete_if_unchanged(&alias.name, expected, self.now())?;
        if deleted {
            for index in &alias.indices {
                self.remove_hint(index, &alias.name, false)?;
            }
        }
        Ok(deleted)
    }

    /// Drops the deleted index `name`, whose descriptor listed `aliased_by`,
    /// from every alias that names it, each by its own swap. An alias left
    /// with no other index keeps naming it, so a search through it says the
    /// index is gone. The count of aliases that may still name it because
    /// they kept changing.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the store fails.
    pub fn detach_from_aliases(
        &mut self,
        name: &str,
        aliased_by: &[Cow<'_, str>],
    ) -> Result<usize, MetaError> {
        let mut left = 0usize;
        for alias in aliased_by {
            let mut done = false;
            for _ in 0..ATTEMPTS {
                let read = self.read(alias)?;
                let Some(record) = read.alias else {
                    done = true;
                    break;
                };
                if record.mutation_pending() && record.protects_hint(name) {
                    self.abort(&record, read.generation)?;
                    continue;
                }
                if !record.indices.iter().any(|i| i == name) {
                    done = true;
                    break;
                }
                let now: Vec<_> = record
                    .indices
                    .iter()
                    .filter(|i| *i != name)
                    .cloned()
                    // ALLOC: the members but one, bounded by the record.
                    .collect();
                if now.is_empty() {
                    // The last index: the alias keeps naming it.
                    done = true;
                    break;
                }
                let next = record.published().with_members(now);
                done = self
                    .descriptors
                    .update_alias(&next, read.generation, &mut self.write_buf)?
                    .is_some();
                if done {
                    break;
                }
            }
            if !done {
                left = left.saturating_add(1);
            }
        }
        Ok(left)
    }

    /// The aliases among `aliased_by`, an index's hint, that name `index`:
    /// one read each, bounded by the hint, never by how many aliases exist.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a malformed register.
    pub fn aliases_of<'n>(
        &mut self,
        index: &str,
        aliased_by: &'n [Cow<'n, str>],
    ) -> Result<Vec<&'n str>, MetaError> {
        // ALLOC: one entry a hinted name, bounded by the hint.
        let mut confirmed = Vec::new();
        for name in aliased_by {
            let resolved = self.descriptors.resolve(name, &mut self.alias_buf)?;
            if let Target::Alias(alias) = resolved.target
                && alias.indices.iter().any(|i| i == index)
            {
                confirmed.push(name.as_ref());
            }
        }
        Ok(confirmed)
    }

    /// Reads `name` for a mutation, waiting up to 250 ms in growing pauses
    /// while another writer's mutation is pending; `None` when it is no
    /// alias, or one still being created. A record still pending after the
    /// wait is returned as it is, and a mutation from it answers `None`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a malformed register.
    pub fn resolve_for_mutation(&mut self, name: &str) -> Result<Option<AliasAt>, MetaError> {
        let mut waited = 0u64;
        let mut pause = 1u64;
        loop {
            let read = self.recover(name)?;
            let Some(alias) = read.alias else {
                return Ok(None);
            };
            if !alias.mutation_pending() || waited >= WAIT_MILLIS {
                return Ok((!alias.creation_pending()).then_some(AliasAt {
                    alias,
                    generation: read.generation,
                }));
            }
            (self.clock.pause)(Duration::from_millis(pause));
            waited = waited.saturating_add(pause);
            pause = pause.saturating_mul(2).min(MAX_PAUSE_MILLIS);
        }
    }

    /// Aborts a reservation past its deadline, up to four times, and reads
    /// what is left.
    fn recover(&mut self, name: &str) -> Result<Read, MetaError> {
        for _ in 0..ATTEMPTS {
            let read = self.read(name)?;
            let Some(alias) = &read.alias else {
                return Ok(read);
            };
            if !alias.mutation_pending() || alias.mutation_until() > self.now() {
                return Ok(read);
            }
            let alias = alias.clone();
            self.abort(&alias, read.generation)?;
        }
        self.read(name)
    }

    /// Prepares every member's hint, then publishes `target`; a failure or
    /// a lost publish undoes the reservation.
    fn publish(
        &mut self,
        pending: &AliasRecord<'static>,
        target: &AliasRecord<'static>,
        reserved: u64,
    ) -> Result<Option<u64>, MetaError> {
        let applied = match self.prepare_and_publish(pending, target, reserved) {
            Ok(applied) => applied,
            Err(e) => {
                // The first failure is the one to report; a reservation the
                // abort cannot undo is recovered after its deadline.
                let _ = self.abort(pending, reserved);
                return Err(e);
            }
        };
        if applied.is_none() {
            self.abort(pending, reserved)?;
        } else if !pending.creation_pending() {
            for index in &pending.indices {
                if !target.indices.contains(index) {
                    self.remove_hint(index, &target.name, false)?;
                }
            }
        }
        Ok(applied)
    }

    fn prepare_and_publish(
        &mut self,
        pending: &AliasRecord<'static>,
        target: &AliasRecord<'static>,
        reserved: u64,
    ) -> Result<Option<u64>, MetaError> {
        // Only the members the mutation adds: one already a member keeps its
        // hint, because every removal first reads the alias, and the
        // reservation still names it. The Java reads every member.
        for index in pending.pending_additions() {
            self.prepare_hint(index, pending, reserved)?;
        }
        self.descriptors
            .update_alias(&target.published(), reserved, &mut self.write_buf)
    }

    /// Undoes a reservation: restores the published record, or deletes a
    /// creation, then removes the hints it added.
    fn abort(&mut self, pending: &AliasRecord<'static>, reserved: u64) -> Result<(), MetaError> {
        if pending.creation_pending() {
            self.descriptors
                .delete_if_unchanged(&pending.name, reserved, self.now())?;
        } else {
            self.descriptors
                .update_alias(&pending.published(), reserved, &mut self.write_buf)?;
        }
        for index in pending.pending_additions() {
            self.remove_hint(index, &pending.name, true)?;
        }
        Ok(())
    }

    /// Adds `pending`'s alias to the hint of `index`, a member it adds, each
    /// swap after checking the reservation still holds. A hint already
    /// there is swapped again, so a removal that read the index before it
    /// loses.
    fn prepare_hint(
        &mut self,
        index: &str,
        pending: &AliasRecord<'static>,
        reserved: u64,
    ) -> Result<(), MetaError> {
        for _ in 0..ATTEMPTS {
            let current = self.descriptors.resolve_raw(index, &mut self.index_buf)?;
            let Target::Index(mut record) = current.target else {
                // ALLOC: the missing member's name, once, for the error.
                return Err(MetaError::MissingMember(index.to_owned()));
            };
            let owner = self
                .descriptors
                .resolve_raw(&pending.name, &mut self.alias_buf)?;
            let holds = matches!(&owner.target, Target::Alias(a) if a.mutation_pending());
            if owner.generation != reserved || !holds {
                return Err(MetaError::Superseded);
            }
            if !record.aliased_by.contains(&pending.name) {
                // ALLOC: one name; a hint that outgrows the register fails
                // the write below, which aborts the mutation.
                record.aliased_by.push(Cow::Borrowed(&pending.name));
            }
            if self
                .descriptors
                .update(&record, current.generation, &mut self.write_buf)?
                .is_some()
            {
                return Ok(());
            }
        }
        Err(MetaError::Contended)
    }

    /// Removes `alias` from `index`'s hint unless the alias, read after the
    /// index, still protects it. With `fence`, the index is swapped even
    /// when the hint is gone, so a preparation that read it before loses.
    /// A hint left behind after four attempts is harmless:
    /// [`AliasMutations::aliases_of`] confirms each.
    fn remove_hint(&mut self, index: &str, alias: &str, fence: bool) -> Result<(), MetaError> {
        for _ in 0..ATTEMPTS {
            let current = self.descriptors.resolve_raw(index, &mut self.index_buf)?;
            let Target::Index(mut record) = current.target else {
                return Ok(());
            };
            let truth = self.descriptors.resolve_raw(alias, &mut self.alias_buf)?;
            if matches!(&truth.target, Target::Alias(a) if a.protects_hint(index)) {
                return Ok(());
            }
            let had = record.aliased_by.len();
            record.aliased_by.retain(|a| a != alias);
            if record.aliased_by.len() == had && !fence {
                return Ok(());
            }
            if self
                .descriptors
                .update(&record, current.generation, &mut self.write_buf)?
                .is_some()
            {
                return Ok(());
            }
        }
        Ok(())
    }
}
