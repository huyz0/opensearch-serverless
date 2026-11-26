//! Adding the indices of a restore to an alias in one mutation (ADR 0065).

use std::borrow::Cow;

use oss_format::AliasRecord;

use crate::alias_mutations::AliasMutations;
use crate::error::MetaError;

/// Times the mutation is retried when another writer swapped the alias first.
const ATTEMPTS: usize = 8;

impl AliasMutations<'_> {
    /// Adds each of `members` (an index and whether the snapshot made it the
    /// alias's write index) to `alias`, creating it when it does not exist.
    /// A member becomes the write index only when the alias has no write
    /// target, explicit or implicit, and then the first that asks does.
    ///
    /// # Errors
    ///
    /// [`MetaError::AlreadyExists`] when the name is an index;
    /// [`MetaError::InvalidArgument`] for a data stream;
    /// [`MetaError::MissingMember`] for a member that is no index;
    /// [`MetaError::Contended`] when the alias kept changing.
    pub fn add_restored(
        &mut self,
        alias: &str,
        members: &[(String, bool)],
    ) -> Result<(), MetaError> {
        for _ in 0..ATTEMPTS {
            if self.add_once(alias, members)? {
                return Ok(());
            }
        }
        Err(MetaError::Contended)
    }

    /// One attempt; `false` when the alias moved under it.
    fn add_once(&mut self, alias: &str, members: &[(String, bool)]) -> Result<bool, MetaError> {
        let first_writer = members.iter().find(|(_, w)| *w).map(|(m, _)| m.as_str());
        let Some(at) = self.resolve_for_mutation(alias)? else {
            // ALLOC: one name a member, at most the restore's bound.
            let names: Vec<&str> = members.iter().map(|(m, _)| m.as_str()).collect();
            let mut record = AliasRecord::new(alias, &names);
            record.write_member = first_writer.map(Cow::Borrowed);
            return match self.create_alias(&record) {
                Ok(_) => Ok(true),
                Err(MetaError::Superseded) => Ok(false),
                Err(e) => Err(e),
            };
        };
        let before = at.alias;
        if before.data_stream {
            return Err(MetaError::InvalidArgument("the name is a data stream"));
        }
        let mut after = before.clone();
        for (member, _) in members {
            if !after.indices.iter().any(|i| i == member) {
                after.indices.push(Cow::Owned(member.clone()));
            }
        }
        // The alias's write target keeps the role, an implicit one made explicit
        // so a second member does not leave the alias with none.
        after.write_member = match before.writes_to() {
            // ALLOC: one name, once a mutation.
            Some(target) => Some(Cow::Owned(target.to_owned())),
            // ALLOC: one name, once a mutation.
            None => first_writer.map(|m| Cow::Owned(m.to_owned())),
        };
        if after == before {
            return Ok(true);
        }
        Ok(self.update_alias(&before, &after, at.generation)?.is_some())
    }
}
