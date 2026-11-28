//! The forest: a ring of recent points, one sampler and one tree per member.

// ARITH: indices are bounded by the validated `Params` (store <= 8,192 points,
// sample <= 2,048, trees <= 100); `seq` is a step count that wraps only after
// 2^64 points and is read through `wrapping_add`.
#![allow(clippy::arithmetic_side_effects)]

use crate::params::Params;
use crate::rng::Rng;
use crate::tree::{Deleted, NIL, Points, Tree};
use crate::workspace::Workspace;

/// A point a tree samples: its place in the ring and its sampling key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Held {
    pub(crate) point: u32,
    pub(crate) key: f64,
}

/// One tree and the points it samples.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Member {
    pub(crate) tree: Tree,
    pub(crate) held: Vec<Held>,
    /// For each ring slot, the index into `held` of the point it holds, or `NIL`.
    pub(crate) slot_of_point: Vec<u32>,
}

/// The forest of one model.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Forest {
    pub(crate) pts: Points,
    pub(crate) members: Vec<Member>,
    /// Points offered so far; the next goes to ring slot `seq % capacity`.
    pub(crate) seq: u64,
    pub(crate) rng: Rng,
    pub(crate) sample_size: usize,
    pub(crate) time_decay: f64,
}

impl Forest {
    pub(crate) fn new(p: &Params) -> Self {
        let cap = p.store_capacity();
        let members = (0..p.trees)
            .map(|_| Member {
                tree: Tree::new(p.node_capacity()),
                held: Vec::with_capacity(p.sample_size),
                slot_of_point: vec![NIL; cap],
            })
            .collect();
        Self {
            pts: Points {
                data: vec![0.0; cap * p.dims()],
                dims: p.dims(),
            },
            members,
            seq: 0,
            rng: Rng::new(p.seed),
            sample_size: p.sample_size,
            time_decay: p.time_decay,
        }
    }

    /// Points the ring holds now.
    pub(crate) fn len(&self) -> usize {
        let cap = self.pts.data.len() / self.pts.dims.max(1);
        (self.seq.min(cap as u64)) as usize
    }

    /// The mean score of `x` over the trees, scaled by `log2(sample_size + 1)`.
    pub(crate) fn score(&self, ws: &mut Workspace, x: &[f32]) -> f64 {
        let sum: f64 = self
            .members
            .iter()
            .map(|m| m.tree.score(&self.pts, ws, x, self.sample_size))
            .sum();
        sum / self.members.len() as f64 * (self.sample_size as f64 + 1.0).log2()
    }

    /// Offers `x` to every tree.
    pub(crate) fn update(&mut self, ws: &mut Workspace, x: &[f32]) {
        let cap = self.pts.data.len() / self.pts.dims;
        let slot = (self.seq % cap as u64) as u32;
        if self.seq >= cap as u64 {
            for m in &mut self.members {
                remove_point(m, &self.pts, slot);
            }
        }
        let at = slot as usize * self.pts.dims;
        self.pts.data[at..at + self.pts.dims].copy_from_slice(x);
        let t = self.seq as f64;
        for m in &mut self.members {
            // A smaller key is likelier kept: ln(-ln u) - decay * t is a monotone form of -ln(u) / exp(decay * t).
            let key = self.rng.open_unit().ln().abs().ln() - self.time_decay * t;
            offer(
                m,
                &mut self.pts,
                ws,
                &mut self.rng,
                slot,
                key,
                self.sample_size,
            );
        }
        self.seq = self.seq.wrapping_add(1);
    }
}

fn offer(
    m: &mut Member,
    pts: &mut Points,
    ws: &mut Workspace,
    rng: &mut Rng,
    slot: u32,
    key: f64,
    sample_size: usize,
) {
    if m.held.len() >= sample_size {
        let mut worst = 0usize;
        for (i, h) in m.held.iter().enumerate() {
            if h.key > m.held[worst].key {
                worst = i;
            }
        }
        if key >= m.held[worst].key {
            return;
        }
        let victim = m.held[worst].point;
        remove_point(m, pts, victim);
    }
    if m.tree.insert(pts, ws, slot, rng) {
        m.slot_of_point[slot as usize] = m.held.len() as u32;
        m.held.push(Held { point: slot, key });
    }
}

/// Removes the ring slot `p` from a member's sample and tree, if it holds it.
fn remove_point(m: &mut Member, pts: &Points, p: u32) {
    let at = m.slot_of_point[p as usize];
    if at == NIL {
        return;
    }
    let at = at as usize;
    m.held.swap_remove(at);
    if let Some(moved) = m.held.get(at) {
        m.slot_of_point[moved.point as usize] = at as u32;
    }
    m.slot_of_point[p as usize] = NIL;
    if let Deleted::Repoint(leaf) = m.tree.delete(pts, p) {
        let x = pts.get(p);
        if let Some(h) = m.held.iter().find(|h| pts.get(h.point) == x) {
            m.tree.repoint(leaf, h.point);
        }
    }
}
