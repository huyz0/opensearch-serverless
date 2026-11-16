//! The unit layout: the cut points that map routing hashes to units
//! (`UnitLayout`). A unit is a run of whole declared shards or a piece of
//! one; ids are never reused.

use oss_format::{MAX_SHARDS, ShardMode, UnitLayoutRecord, UnitRecord};

use super::RoutingError;
use super::hash::{declared_shard, first_hash_of, last_hash_of, routing_hash};

/// One unit and the hash range it covers, both ends inclusive and unsigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Unit {
    /// The unit id, unique for the index's life.
    pub id: u32,
    /// The first hash it covers.
    pub first: u64,
    /// The last hash it covers.
    pub last: u64,
}

impl Unit {
    /// Whether this unit covers `hash`.
    pub fn covers(&self, hash: u64) -> bool {
        self.first <= hash && hash <= self.last
    }

    fn record(self) -> UnitRecord {
        UnitRecord {
            // Ids come from records (never negative) or from `next_unit_id`,
            // which is an `i32` when read.
            id: i32::try_from(self.id).unwrap_or(i32::MAX),
            first: self.first,
            last: self.last,
        }
    }

    fn from_record(unit: UnitRecord) -> Result<Self, RoutingError> {
        Ok(Self {
            id: u32::try_from(unit.id)
                .map_err(|_| RoutingError::Invalid("a unit id is negative"))?,
            first: unit.first,
            last: unit.last,
        })
    }
}

/// A planned split: the unit it retires and the two halves that replace
/// it; `left` ends right before `right` starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Split {
    /// The unit being split.
    pub retired: Unit,
    /// The lower half.
    pub left: Unit,
    /// The upper half.
    pub right: Unit,
}

/// An index's unit layout, checked to cover the whole hash space and keep
/// the shape rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitLayout {
    mode: ShardMode,
    declared_shards: u32,
    // Sorted by first hash; the first starts at 0 and the last ends at the
    // largest hash, with no gap or overlap.
    units: Vec<Unit>,
    next_unit_id: u32,
    pending: Option<Split>,
}

impl UnitLayout {
    /// A layout from its parts, in any order, checked as the Java's
    /// constructor checks it.
    ///
    /// # Errors
    ///
    /// [`RoutingError::Invalid`] when there is no unit, a count is zero, an
    /// id repeats or is not below `next_unit_id`, the units leave a gap or
    /// overlap, a unit holds part of a declared shard together with another,
    /// or `pending` does not split a unit of the layout into two new halves.
    pub fn new(
        mode: ShardMode,
        declared_shards: u32,
        mut units: Vec<Unit>,
        next_unit_id: u32,
        pending: Option<Split>,
    ) -> Result<Self, RoutingError> {
        if declared_shards < 1 {
            return Err(RoutingError::Invalid("declared shards must be positive"));
        }
        if units.is_empty() {
            return Err(RoutingError::Invalid("a layout needs at least one unit"));
        }
        units.sort_by_key(|unit| unit.first);
        let mut expected = Some(0u64);
        for (at, unit) in units.iter().enumerate() {
            let repeated = units
                .get(..at)
                .is_some_and(|before| before.iter().any(|u| u.id == unit.id));
            if unit.id >= next_unit_id || repeated {
                return Err(RoutingError::Invalid(
                    "a unit has a duplicate or out-of-range id",
                ));
            }
            if Some(unit.first) != expected || unit.first > unit.last {
                return Err(RoutingError::Invalid("the units leave a gap or overlap"));
            }
            check_shape(unit, declared_shards)?;
            expected = unit.last.checked_add(1);
        }
        if expected.is_some() {
            return Err(RoutingError::Invalid(
                "the last unit does not reach the end of the hash space",
            ));
        }
        if let Some(p) = &pending {
            let ids_taken = units
                .iter()
                .any(|u| u.id == p.left.id || u.id == p.right.id);
            if !units.contains(&p.retired)
                || p.left.first != p.retired.first
                || p.right.last != p.retired.last
                || p.left.last.checked_add(1) != Some(p.right.first)
                || ids_taken
                || p.left.id == p.right.id
                || p.left.id.max(p.right.id) >= next_unit_id
            {
                return Err(RoutingError::Invalid(
                    "the pending split does not split a unit of this layout into two new halves",
                ));
            }
            check_shape(&p.left, declared_shards)?;
            check_shape(&p.right, declared_shards)?;
        }
        Ok(Self {
            mode,
            declared_shards,
            units,
            next_unit_id,
            pending,
        })
    }

    /// The layout a new index starts with: fixed, one unit per declared
    /// shard with ids `0..n`; adaptive, one unit with id 0.
    ///
    /// # Errors
    ///
    /// [`RoutingError::Invalid`] when `declared_shards` is zero.
    pub fn initial(mode: ShardMode, declared_shards: u32) -> Result<Self, RoutingError> {
        let units = match mode {
            // ALLOC: one unit.
            ShardMode::Adaptive => vec![Unit {
                id: 0,
                first: 0,
                last: u64::MAX,
            }],
            ShardMode::Fixed => (0..declared_shards)
                .map(|shard| Unit {
                    id: shard,
                    first: first_hash_of(shard, declared_shards).unwrap_or(0),
                    last: last_hash_of(shard, declared_shards).unwrap_or(u64::MAX),
                })
                // ALLOC: one unit per declared shard, bounded by the caller's count.
                .collect(),
        };
        let next = match mode {
            ShardMode::Adaptive => 1,
            ShardMode::Fixed => declared_shards,
        };
        Self::new(mode, declared_shards, units, next, None)
    }

    /// A layout from the record a descriptor holds.
    ///
    /// # Errors
    ///
    /// As [`UnitLayout::new`].
    pub fn from_record(record: &UnitLayoutRecord) -> Result<Self, RoutingError> {
        let count =
            |n: i32| u32::try_from(n).map_err(|_| RoutingError::Invalid("a count is negative"));
        let units = record
            .units
            .iter()
            .map(|u| Unit::from_record(*u))
            // ALLOC: the record's units, bounded by MAX_SHARDS when it was read.
            .collect::<Result<Vec<_>, _>>()?;
        let pending = match record.pending {
            Some([retired, left, right]) => Some(Split {
                retired: Unit::from_record(retired)?,
                left: Unit::from_record(left)?,
                right: Unit::from_record(right)?,
            }),
            None => None,
        };
        Self::new(
            record.mode,
            count(record.declared_shards)?,
            units,
            count(record.next_unit_id)?,
            pending,
        )
    }

    /// Reads and checks a layout from its JSON object bytes.
    ///
    /// # Errors
    ///
    /// [`RoutingError::Format`] when the bytes are not a layout record, and
    /// as [`UnitLayout::new`].
    pub fn parse(bytes: &[u8]) -> Result<Self, RoutingError> {
        Self::from_record(&UnitLayoutRecord::parse(bytes)?)
    }

    /// The record the Java writes for this layout.
    pub fn to_record(&self) -> UnitLayoutRecord {
        let count = |n: u32| i32::try_from(n).unwrap_or(i32::MAX);
        UnitLayoutRecord {
            mode: self.mode,
            declared_shards: count(self.declared_shards),
            next_unit_id: count(self.next_unit_id),
            // ALLOC: one record per unit, as many as the layout holds.
            units: self.units.iter().map(|u| u.record()).collect(),
            pending: self
                .pending
                .map(|p| [p.retired.record(), p.left.record(), p.right.record()]),
        }
    }

    /// The index's shard mode.
    pub fn mode(&self) -> ShardMode {
        self.mode
    }

    /// The user's `number_of_shards`, which every user-facing count reports.
    pub fn declared_shards(&self) -> u32 {
        self.declared_shards
    }

    /// The units, in hash order.
    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    /// How many live units there are.
    pub fn unit_count(&self) -> usize {
        self.units.len()
    }

    /// The id the next new unit gets: also the index's shard slots, which
    /// the descriptor writes as `number_of_shards`.
    pub fn next_unit_id(&self) -> u32 {
        self.next_unit_id
    }

    /// Whether the slots fit the descriptor's bound ([`MAX_SHARDS`]): a
    /// layout past it cannot be written into a descriptor, so a split that
    /// would need more slots is refused before it starts.
    pub fn within_shard_limit(&self) -> bool {
        self.next_unit_id <= MAX_SHARDS
    }

    /// The unit covering `hash`.
    pub fn unit_for(&self, hash: u64) -> &Unit {
        // The last unit whose first hash is at or below the hash; the first
        // unit starts at 0, so there is one.
        let at = self
            .units
            .partition_point(|u| u.first <= hash)
            .saturating_sub(1);
        // `units` is never empty (checked on construction).
        self.units.get(at).unwrap_or(&self.units[0])
    }

    /// The unit a routing value goes to: the document's routing, or its id
    /// when it has none.
    pub fn unit_for_routing(&self, routing: &str) -> &Unit {
        self.unit_for(routing_hash(routing))
    }

    /// The units holding a declared shard's documents, in hash order: one
    /// while it is whole inside a unit, more once it is halved. None when
    /// `declared_shard` is not below the declared count.
    pub fn units_of(&self, declared_shard: u32) -> impl Iterator<Item = &Unit> {
        let range = first_hash_of(declared_shard, self.declared_shards)
            .zip(last_hash_of(declared_shard, self.declared_shards));
        self.units
            .iter()
            .filter(move |u| range.is_some_and(|(first, last)| u.last >= first && u.first <= last))
    }

    /// Whether a search of one declared shard needs a hash filter on
    /// `unit`: true when the unit holds other declared shards' documents.
    pub fn needs_filter(&self, unit: &Unit, declared_shard: u32) -> bool {
        declared_shard_of(unit.first, self.declared_shards) != declared_shard
            || declared_shard_of(unit.last, self.declared_shards) != declared_shard
    }

    /// Plans splitting a unit in two, without changing this layout: a run
    /// of declared shards is cut at the boundary nearest its middle, a piece
    /// of one is halved by hash.
    ///
    /// # Errors
    ///
    /// [`RoutingError::SplitPending`] when a split is already pending;
    /// [`RoutingError::Invalid`] when there is no such unit or it covers a
    /// single hash.
    pub fn plan_split(&self, unit_id: u32) -> Result<Split, RoutingError> {
        if self.pending.is_some() {
            return Err(RoutingError::SplitPending);
        }
        let unit = *self.unit(unit_id)?;
        let first_shard = declared_shard_of(unit.first, self.declared_shards);
        let last_shard = declared_shard_of(unit.last, self.declared_shards);
        let cut = if last_shard > first_shard {
            let middle = first_shard
                .saturating_add(last_shard.saturating_sub(first_shard).saturating_add(1) / 2);
            first_hash_of(middle, self.declared_shards).unwrap_or(unit.last)
        } else {
            if unit.first == unit.last {
                return Err(RoutingError::Invalid(
                    "a unit covering a single hash cannot be split",
                ));
            }
            unit.first
                .wrapping_add(unit.last.wrapping_sub(unit.first) >> 1)
                .wrapping_add(1)
        };
        let left_id = self.next_unit_id;
        Ok(Split {
            retired: unit,
            left: Unit {
                id: left_id,
                first: unit.first,
                last: cut.wrapping_sub(1),
            },
            right: Unit {
                id: left_id.saturating_add(1),
                first: cut,
                last: unit.last,
            },
        })
    }

    /// The split planned but not yet applied: its halves' ids are reserved
    /// slots, but nothing routes to them until it is applied.
    pub fn pending(&self) -> Option<&Split> {
        self.pending.as_ref()
    }

    /// This layout with `split` recorded as pending, its halves' ids reserved.
    ///
    /// # Errors
    ///
    /// As [`UnitLayout::new`], when `split` was not planned from this layout.
    pub fn with_pending(&self, split: Split) -> Result<Self, RoutingError> {
        Self::new(
            self.mode,
            self.declared_shards,
            self.units.clone(),
            self.after(&split),
            Some(split),
        )
    }

    /// This layout without its pending split; the reserved ids stay used.
    pub fn without_pending(&self) -> Self {
        Self {
            pending: None,
            ..self.clone()
        }
    }

    /// This layout with `split` applied; a pending split, if it is this
    /// one, is cleared.
    ///
    /// # Errors
    ///
    /// [`RoutingError::Invalid`] when the split's retired unit is not in
    /// this layout, and as [`UnitLayout::new`].
    pub fn apply(&self, split: Split) -> Result<Self, RoutingError> {
        if !self.units.contains(&split.retired) {
            return Err(RoutingError::Invalid(
                "the split's retired unit is not in this layout",
            ));
        }
        let mut next: Vec<Unit> = self
            .units
            .iter()
            .copied()
            .filter(|u| *u != split.retired)
            // ALLOC: the layout's units, then one more.
            .collect();
        next.push(split.left);
        next.push(split.right);
        let still_pending = self.pending.filter(|p| *p != split);
        Self::new(
            self.mode,
            self.declared_shards,
            next,
            self.after(&split),
            still_pending,
        )
    }

    /// This layout with two neighbouring units merged into one new unit.
    ///
    /// # Errors
    ///
    /// [`RoutingError::Invalid`] when there is no such unit, they are not
    /// neighbours, or the merge would break the shape rule;
    /// [`RoutingError::SplitPending`] when either has a split pending.
    pub fn merge(&self, lower_id: u32, upper_id: u32) -> Result<Self, RoutingError> {
        let lower = *self.unit(lower_id)?;
        let upper = *self.unit(upper_id)?;
        if lower.last.checked_add(1) != Some(upper.first) {
            return Err(RoutingError::Invalid("the units are not neighbours"));
        }
        if self
            .pending
            .is_some_and(|p| p.retired == lower || p.retired == upper)
        {
            return Err(RoutingError::SplitPending);
        }
        let mut next: Vec<Unit> = self
            .units
            .iter()
            .copied()
            .filter(|u| *u != lower && *u != upper)
            // ALLOC: the layout's units less one, bounded by its count.
            .collect();
        next.push(Unit {
            id: self.next_unit_id,
            first: lower.first,
            last: upper.last,
        });
        Self::new(
            self.mode,
            self.declared_shards,
            next,
            self.next_unit_id.saturating_add(1),
            self.pending,
        )
    }

    /// The unit with id `unit_id`.
    ///
    /// # Errors
    ///
    /// [`RoutingError::Invalid`] when this layout has no such unit.
    pub fn unit(&self, unit_id: u32) -> Result<&Unit, RoutingError> {
        self.units
            .iter()
            .find(|u| u.id == unit_id)
            .ok_or(RoutingError::Invalid("no such unit in this layout"))
    }

    fn after(&self, split: &Split) -> u32 {
        self.next_unit_id
            .max(split.left.id.max(split.right.id).saturating_add(1))
    }
}

fn declared_shard_of(hash: u64, declared_shards: u32) -> u32 {
    declared_shard(hash, declared_shards)
}

/// A unit is a piece of one declared shard, exactly one, or a run of whole
/// ones.
fn check_shape(unit: &Unit, declared_shards: u32) -> Result<(), RoutingError> {
    let first_shard = declared_shard_of(unit.first, declared_shards);
    let last_shard = declared_shard_of(unit.last, declared_shards);
    if first_shard == last_shard
        || (Some(unit.first) == first_hash_of(first_shard, declared_shards)
            && Some(unit.last) == last_hash_of(last_shard, declared_shards))
    {
        return Ok(());
    }
    Err(RoutingError::Invalid(
        "a unit holds part of a declared shard together with another declared shard",
    ))
}
