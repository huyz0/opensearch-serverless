//! The checkpoint: a model's whole state as bytes, and back.
//!
//! Layout (little endian): magic `OSAD`, a version byte, the parameters, the
//! model's counters, the thresholder, the shingle, the forest (the ring's
//! live rows, then each tree's arena, free list and sample), and a 64-bit
//! FNV-1a checksum of everything before it. A blob read back is untrusted:
//! the checksum is checked before anything is allocated, every length comes
//! from the validated parameters, and each tree is verified structurally.

use std::fmt;

use crate::forest::{Forest, Held, Member};
use crate::model::Model;
use crate::params::{Imputation, Params};
use crate::rng::Rng;
use crate::tree::{NIL, Node, Points, Tree};

const MAGIC: &[u8; 4] = b"OSAD";
/// The layout version this build writes and reads.
pub const VERSION: u8 = 1;

/// Why a checkpoint was not restored; the model then cold-starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// Not a checkpoint (too short, wrong magic).
    NotACheckpoint,
    /// Written by a layout this build does not read.
    Version(u8),
    /// The checksum does not match.
    Corrupt,
    /// The parameters inside are out of range.
    Params(String),
    /// The state is internally inconsistent.
    Inconsistent(&'static str),
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotACheckpoint => f.write_str("not an anomaly model checkpoint"),
            Self::Version(v) => write!(f, "checkpoint version {v} is not supported"),
            Self::Corrupt => f.write_str("checkpoint checksum mismatch"),
            Self::Params(p) => write!(f, "checkpoint parameters invalid: {p}"),
            Self::Inconsistent(what) => write!(f, "checkpoint inconsistent: {what}"),
        }
    }
}

impl std::error::Error for RestoreError {}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

struct In<'a>(&'a [u8]);

impl In<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], RestoreError> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(RestoreError::Inconsistent("truncated"))?;
        let out = *head;
        self.0 = rest;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, RestoreError> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, RestoreError> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32, RestoreError> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64, RestoreError> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32, RestoreError> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn f64(&mut self) -> Result<f64, RestoreError> {
        Ok(f64::from_le_bytes(self.take()?))
    }
    fn usize(&mut self) -> Result<usize, RestoreError> {
        Ok(self.u32()? as usize)
    }
    fn finite32(&mut self) -> Result<f32, RestoreError> {
        let v = self.f32()?;
        v.is_finite()
            .then_some(v)
            .ok_or(RestoreError::Inconsistent("a number is not finite"))
    }
    fn finite64(&mut self) -> Result<f64, RestoreError> {
        let v = self.f64()?;
        v.is_finite()
            .then_some(v)
            .ok_or(RestoreError::Inconsistent("a number is not finite"))
    }
}

impl Model {
    /// The model's whole state as a checkpoint.
    pub fn checkpoint(&self) -> Vec<u8> {
        let p = &self.params;
        let mut o = Out(Vec::new());
        o.0.extend_from_slice(MAGIC);
        o.u8(VERSION);
        o.u32(p.trees as u32);
        o.u32(p.sample_size as u32);
        o.u32(p.shingle_size as u32);
        o.u32(p.features as u32);
        o.f64(p.time_decay);
        o.u64(p.seed);
        o.u32(p.min_train_samples as u32);
        o.u64(p.max_gap);
        match &p.imputation {
            Imputation::Zero => o.u8(0),
            Imputation::Previous => o.u8(1),
            Imputation::Fixed(v) => {
                o.u8(2);
                v.iter().for_each(|x| o.f32(*x));
            }
        }
        o.u8(u8::from(self.last_fed_slot.is_some()));
        o.u64(self.last_fed_slot.unwrap_or(0));
        o.u64(self.points_seen);
        o.f64(self.thresh.mean);
        o.f64(self.thresh.var);
        o.u64(self.thresh.n);
        o.f64(self.thresh.rate);
        let s = &self.shingle;
        o.u32(s.filled as u32);
        o.u32(s.head as u32);
        o.u8(u8::from(s.has_last));
        s.windows.iter().for_each(|x| o.f32(*x));
        s.last.iter().for_each(|x| o.f32(*x));
        let f = &self.forest;
        o.u64(f.seq);
        f.rng.state().iter().for_each(|w| o.u64(*w));
        let rows = f.len().saturating_mul(f.pts.dims);
        f.pts.data[..rows].iter().for_each(|x| o.f32(*x));
        for m in &f.members {
            o.u32(m.tree.root);
            o.u32(m.tree.free.len() as u32);
            m.tree.free.iter().for_each(|i| o.u32(*i));
            for n in &m.tree.nodes {
                o.u32(n.parent);
                o.u32(n.left);
                o.u32(n.right);
                o.u32(n.mass);
                o.u32(n.point);
                o.f32(n.cut_val);
                o.u16(n.cut_dim);
            }
            o.u32(m.held.len() as u32);
            for h in &m.held {
                o.u32(h.point);
                o.f64(h.key);
            }
        }
        let sum = fnv(&o.0);
        o.u64(sum);
        o.0
    }

    /// Restores a model from [`Model::checkpoint`]'s bytes, bit-identical to it.
    pub fn restore(bytes: &[u8]) -> Result<Self, RestoreError> {
        let body_len = bytes
            .len()
            .checked_sub(8)
            .filter(|n| *n >= 5)
            .ok_or(RestoreError::NotACheckpoint)?;
        let (body, sum) = bytes.split_at(body_len);
        if &body[..4] != MAGIC {
            return Err(RestoreError::NotACheckpoint);
        }
        if body[4] != VERSION {
            return Err(RestoreError::Version(body[4]));
        }
        let mut stored = [0u8; 8];
        stored.copy_from_slice(sum);
        if fnv(body) != u64::from_le_bytes(stored) {
            return Err(RestoreError::Corrupt);
        }
        let mut i = In(&body[5..]);
        let mut p = Params::new(1, 0);
        p.trees = i.usize()?;
        p.sample_size = i.usize()?;
        p.shingle_size = i.usize()?;
        p.features = i.usize()?;
        p.time_decay = i.f64()?;
        p.seed = i.u64()?;
        p.min_train_samples = i.usize()?;
        p.max_gap = i.u64()?;
        p.imputation = match i.u8()? {
            0 => Imputation::Zero,
            1 => Imputation::Previous,
            2 if p.features <= crate::params::MAX_FEATURES => {
                let mut v = Vec::with_capacity(p.features);
                for _ in 0..p.features {
                    v.push(i.f32()?);
                }
                Imputation::Fixed(v)
            }
            _ => return Err(RestoreError::Inconsistent("imputation")),
        };
        p.validate().map_err(|e| RestoreError::Params(e.0))?;
        let mut m = Model::assemble(p);
        read_state(&mut i, &mut m)?;
        if !i.0.is_empty() {
            return Err(RestoreError::Inconsistent("trailing bytes"));
        }
        Ok(m)
    }
}

fn read_state(i: &mut In<'_>, m: &mut Model) -> Result<(), RestoreError> {
    let bad = RestoreError::Inconsistent;
    let present = i.u8()? == 1;
    let slot = i.u64()?;
    m.last_fed_slot = present.then_some(slot);
    m.points_seen = i.u64()?;
    m.thresh.mean = i.finite64()?;
    m.thresh.var = i.finite64()?;
    m.thresh.n = i.u64()?;
    m.thresh.rate = i.finite64()?;
    if m.thresh.var < 0.0 || !(m.thresh.rate > 0.0 && m.thresh.rate <= 1.0) {
        return Err(bad("thresholder"));
    }
    m.shingle.filled = i.usize()?;
    m.shingle.head = i.usize()?;
    m.shingle.has_last = i.u8()? == 1;
    if m.shingle.filled > m.shingle.size || m.shingle.head >= m.shingle.size {
        return Err(bad("shingle"));
    }
    for x in &mut m.shingle.windows {
        *x = i.finite32()?;
    }
    for x in &mut m.shingle.last {
        *x = i.finite32()?;
    }
    let f: &mut Forest = &mut m.forest;
    f.seq = i.u64()?;
    let mut state = [0u64; 4];
    for w in &mut state {
        *w = i.u64()?;
    }
    f.rng = Rng::from_state(state).ok_or(bad("generator"))?;
    let rows = f.len();
    let dims = f.pts.dims;
    for x in &mut f.pts.data[..rows.saturating_mul(dims)] {
        *x = i.finite32()?;
    }
    let sample = m.params.sample_size;
    for member in &mut f.members {
        read_member(i, member, &f.pts, rows, dims, sample)?;
    }
    Ok(())
}

fn read_member(
    i: &mut In<'_>,
    m: &mut Member,
    pts: &Points,
    rows: usize,
    dims: usize,
    sample: usize,
) -> Result<(), RestoreError> {
    let bad = RestoreError::Inconsistent;
    let root = i.u32()?;
    let free_len = i.usize()?;
    if free_len > m.tree.nodes.len() {
        return Err(bad("free list"));
    }
    let mut free = Vec::with_capacity(m.tree.nodes.len());
    for _ in 0..free_len {
        free.push(i.u32()?);
    }
    let mut nodes = Vec::with_capacity(m.tree.nodes.len());
    for _ in 0..m.tree.nodes.len() {
        nodes.push(Node {
            parent: i.u32()?,
            left: i.u32()?,
            right: i.u32()?,
            mass: i.u32()?,
            point: i.u32()?,
            cut_val: i.finite32()?,
            cut_dim: i.u16()?,
        });
    }
    let held_len = i.usize()?;
    if held_len > sample {
        return Err(bad("sample"));
    }
    let tree = Tree { nodes, free, root };
    let mass = tree.verify(rows, dims).ok_or(bad("tree"))?;
    if mass as usize != held_len {
        return Err(bad("tree mass"));
    }
    m.slot_of_point.fill(NIL);
    m.held.clear();
    for k in 0..held_len {
        let point = i.u32()?;
        let key = i.finite64()?;
        let at = m
            .slot_of_point
            .get_mut(point as usize)
            .ok_or(bad("sample point"))?;
        if *at != NIL || point as usize >= rows {
            return Err(bad("sample point"));
        }
        *at = k as u32;
        m.held.push(Held { point, key });
    }
    // Every sampled point sits in a leaf of equal values, and each leaf's mass is
    // the number of sampled points under it.
    let mut counts = vec![0u32; tree.nodes.len()];
    for h in &m.held {
        let leaf = tree
            .leaf_of(pts, h.point)
            .ok_or(bad("sampled point not in the tree"))?;
        counts[leaf as usize] = counts[leaf as usize].saturating_add(1);
    }
    if tree
        .nodes
        .iter()
        .zip(&counts)
        .any(|(n, c)| n.is_leaf() && n.mass > 0 && n.mass != *c)
    {
        return Err(bad("leaf mass"));
    }
    m.tree = tree;
    Ok(())
}
