//! The role as a bulk target: items bound for a shard held here are applied
//! and staged as a group, the rest are forwarded to their owner.

use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned, Target};
use oss_writer::index::{Refused, Written};

use super::{Failure, WriterRole};

/// A bulk request's view of a [`WriterRole`].
pub struct RoleTarget<'r> {
    role: &'r WriterRole,
}

impl<'r> RoleTarget<'r> {
    /// A target over `role`.
    pub fn new(role: &'r WriterRole) -> Self {
        Self { role }
    }
}

impl Target for RoleTarget<'_> {
    fn index(&mut self, name: &str) -> Option<IndexInfo> {
        self.role.routes.index(name)
    }

    fn owner(&mut self, index: &IndexInfo, shard: u32) -> Owner {
        self.role.owner(index, shard)
    }

    /// One item, staged on its own; a bulk applies groups through
    /// [`Target::apply_group`], so this is the one-item case. A write that
    /// could not be made durable is reported as a conflict carrying why.
    fn apply(
        &mut self,
        index: &IndexInfo,
        shard: u32,
        item: &Planned<'_>,
    ) -> Result<Written, Refused> {
        match self
            .role
            .group(index, shard, std::slice::from_ref(item))
            .pop()
        {
            Some(Ok(written)) => Ok(written),
            Some(Err(Failure::Refused(refused))) => Err(refused),
            Some(Err(Failure::Unavailable(reason))) => Err(Refused::Conflict(reason)),
            None => Err(Refused::Conflict("no outcome".to_owned())),
        }
    }

    fn apply_group(
        &mut self,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        self.role.apply_group(index, shard, group)
    }

    fn refresh(&mut self, index: &IndexInfo, shard: u32) -> Result<(), String> {
        self.role.refresh(index, shard)
    }

    fn forward(
        &mut self,
        node: &str,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        self.role.routes.forward(node, index, shard, group)
    }
}
