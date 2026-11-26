//! Confirming the aliases of the indices a take captures against each
//! alias's own register (ADR 0065).

use std::collections::BTreeMap;

use oss_format::AliasCapture;

use crate::descriptors::{DescriptorStore, Target};
use crate::pit::CaptureError;
use crate::snapshot_capture::DescribedIndex;
use crate::snapshot_take::SnapshotTaker;

/// An alias's members and its explicit write member.
type Confirmed = (Vec<String>, Option<String>);

impl SnapshotTaker {
    /// Sets each index's confirmed aliases from its hint. Cost: one GET for
    /// each distinct alias named, however many indices name it.
    ///
    /// # Errors
    ///
    /// [`CaptureError`] for a register that cannot be read.
    pub fn capture_aliases(
        &self,
        descriptors: &DescriptorStore,
        indices: &mut [DescribedIndex],
        buf: &mut [u8],
    ) -> Result<(), CaptureError> {
        // Each distinct alias once: its members and its explicit write member.
        // ALLOC: one entry a distinct alias, at most MAX_ALIASES an index.
        let mut read: BTreeMap<&str, Option<Confirmed>> = BTreeMap::new();
        for name in indices.iter().flat_map(|i| i.hint.iter()) {
            if read.contains_key(name.as_str()) {
                continue;
            }
            let found = match descriptors.resolve(name, buf)?.target {
                // A data stream is not an alias; a creation not yet published reads as absent.
                Target::Alias(a) if !a.data_stream && !a.creation_pending() => Some((
                    // ALLOC: the members, bounded by the register's size.
                    a.indices.iter().map(|m| m.to_string()).collect(),
                    a.kept_write_member().map(str::to_owned),
                )),
                _ => None,
            };
            read.insert(name.as_str(), found);
        }
        let confirmed: Vec<Vec<AliasCapture>> = indices
            .iter()
            .map(|index| {
                index
                    .hint
                    .iter()
                    .filter_map(|name| {
                        let (members, write) = read.get(name.as_str())?.as_ref()?;
                        members.contains(&index.name).then(|| AliasCapture {
                            name: name.clone(),
                            write: write.as_deref() == Some(index.name.as_str()),
                        })
                    })
                    // ALLOC: one entry an alias, at most MAX_ALIASES.
                    .collect()
            })
            // ALLOC: one list an index, at most the take's bound.
            .collect();
        for (index, aliases) in indices.iter_mut().zip(confirmed) {
            index.aliases = aliases;
        }
        Ok(())
    }
}
