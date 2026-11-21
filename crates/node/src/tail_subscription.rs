//! One reader's subscription to a shard's pushes, as the writer counts it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) struct Subscription {
    pub(crate) id: i64,
    /// When the grant runs out, in milliseconds on the clock.
    pub(crate) grant_until: i64,
    /// The shard's position through which the node has every record.
    pub(crate) confirmed: u64,
    /// The number the next push of this shard to the node takes.
    pub(crate) next_send: u64,
    /// The lowest push number not yet answered.
    pub(crate) next_to_confirm: u64,
    /// Pushes sent and not yet counted, by number: the position each reaches.
    pub(crate) sent: BTreeMap<u64, u64>,
    /// Pushes answered out of order, waiting for an earlier one.
    pub(crate) answered: BTreeSet<u64>,
}

impl Subscription {
    pub(crate) fn new(id: i64, grant_until: i64, confirmed: u64) -> Self {
        Self {
            id,
            grant_until,
            confirmed,
            next_send: 0,
            next_to_confirm: 0,
            sent: BTreeMap::new(),
            answered: BTreeSet::new(),
        }
    }

    pub(crate) fn answered_push(&mut self, number: u64) {
        // A push already counted, or never sent, is not in `sent`.
        if !self.sent.contains_key(&number) {
            return;
        }
        self.answered.insert(number);
        while self.answered.remove(&self.next_to_confirm) {
            if let Some(reached) = self.sent.remove(&self.next_to_confirm) {
                self.confirmed = self.confirmed.max(reached);
            }
            self.next_to_confirm = self.next_to_confirm.saturating_add(1);
        }
    }
}

/// A push's place in the subscription it was sent under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sent {
    /// The subscription's id.
    pub id: i64,
    /// The push's number within it.
    pub number: u64,
}

#[derive(Default)]
pub(crate) struct Serving {
    pub(crate) by_node: HashMap<String, Subscription>,
    pub(crate) next_id: i64,
}
