//! One random cut tree in a fixed arena: insert, delete and score a point.
//!
//! Nodes hold no bounding box. The boxes a cut or a score needs are computed
//! bottom-up into the caller's [`Workspace`] by one pass over the tree, which
//! keeps a model's memory independent of the point's width at the cost of a
//! pass per operation.

// Every index is a node, point or dimension number bounded by the validated
// `Params` (nodes <= 4,095, dims <= 512, store <= 8,192), and every mass is at
// most `sample_size`.
// ARITH: no product or sum here can overflow a u32 or usize, by those bounds.
#![allow(clippy::arithmetic_side_effects)]

use crate::rng::Rng;
use crate::workspace::Workspace;

/// "No node".
pub(crate) const NIL: u32 = u32::MAX;
/// The `cut_dim` of a leaf.
pub(crate) const LEAF: u16 = u16::MAX;

/// A node of the arena: a leaf holds a point, an inner node a cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) parent: u32,
    pub(crate) left: u32,
    pub(crate) right: u32,
    /// Points under this node, duplicates counted.
    pub(crate) mass: u32,
    /// A leaf's point: an index into the ring of recent points.
    pub(crate) point: u32,
    pub(crate) cut_val: f32,
    pub(crate) cut_dim: u16,
}

impl Node {
    pub(crate) const EMPTY: Node = Node {
        parent: NIL,
        left: NIL,
        right: NIL,
        mass: 0,
        point: NIL,
        cut_val: 0.0,
        cut_dim: LEAF,
    };

    pub(crate) fn is_leaf(&self) -> bool {
        self.cut_dim == LEAF
    }
}

/// The ring of recent points every tree samples from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Points {
    pub(crate) data: Vec<f32>,
    pub(crate) dims: usize,
}

impl Points {
    pub(crate) fn get(&self, i: u32) -> &[f32] {
        let at = i as usize * self.dims;
        &self.data[at..at + self.dims]
    }
}

/// What a delete left for the caller to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Deleted {
    /// The point is gone.
    Gone,
    /// The point's leaf still holds equal points and names the deleted one: repoint it.
    Repoint(u32),
    /// The tree does not hold the point (a corrupt state).
    Missing,
}

/// One tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tree {
    pub(crate) nodes: Vec<Node>,
    pub(crate) free: Vec<u32>,
    pub(crate) root: u32,
}

impl Tree {
    pub(crate) fn new(node_capacity: usize) -> Self {
        let mut free = Vec::with_capacity(node_capacity);
        free.extend((0..node_capacity as u32).rev());
        Self {
            nodes: vec![Node::EMPTY; node_capacity],
            free,
            root: NIL,
        }
    }

    fn alloc(&mut self, node: Node) -> Option<u32> {
        let i = self.free.pop()?;
        self.nodes[i as usize] = node;
        Some(i)
    }

    fn release(&mut self, i: u32) {
        self.nodes[i as usize] = Node::EMPTY;
        self.free.push(i);
    }

    /// Computes every node's bounding box into `ws`.
    fn boxes(&self, pts: &Points, ws: &mut Workspace) {
        let dims = pts.dims;
        let Workspace {
            lo,
            hi,
            order,
            stack,
            ..
        } = ws;
        order.clear();
        stack.clear();
        if self.root == NIL {
            return;
        }
        stack.push(self.root);
        while let Some(n) = stack.pop() {
            order.push(n);
            let node = &self.nodes[n as usize];
            if !node.is_leaf() {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        for &n in order.iter().rev() {
            let node = self.nodes[n as usize];
            let at = n as usize * dims;
            if node.is_leaf() {
                let p = pts.get(node.point);
                lo[at..at + dims].copy_from_slice(p);
                hi[at..at + dims].copy_from_slice(p);
            } else {
                let (l, r) = (node.left as usize * dims, node.right as usize * dims);
                for d in 0..dims {
                    lo[at + d] = lo[l + d].min(lo[r + d]);
                    hi[at + d] = hi[l + d].max(hi[r + d]);
                }
            }
        }
    }

    fn add_mass_up(&mut self, mut n: u32, delta: i64) {
        while n != NIL {
            let node = &mut self.nodes[n as usize];
            node.mass = (i64::from(node.mass) + delta) as u32;
            n = node.parent;
        }
    }

    /// Inserts point `p` (already in `pts`). Returns false when the arena is full.
    pub(crate) fn insert(
        &mut self,
        pts: &Points,
        ws: &mut Workspace,
        p: u32,
        rng: &mut Rng,
    ) -> bool {
        let x = pts.get(p);
        let dims = pts.dims;
        if self.root == NIL {
            let Some(leaf) = self.alloc(Node {
                mass: 1,
                point: p,
                ..Node::EMPTY
            }) else {
                return false;
            };
            self.root = leaf;
            return true;
        }
        self.boxes(pts, ws);
        let mut n = self.root;
        loop {
            let at = n as usize * dims;
            let (lo, hi) = (&ws.lo[at..at + dims], &ws.hi[at..at + dims]);
            let mut total = 0.0f64;
            for d in 0..dims {
                total += f64::from(hi[d].max(x[d]) - lo[d].min(x[d]));
            }
            if total == 0.0 {
                // x equals every point under n, so n is a leaf of equal points
                self.nodes[n as usize].mass += 1;
                let parent = self.nodes[n as usize].parent;
                self.add_mass_up(parent, 1);
                return true;
            }
            let pick = rng.open_unit() * total;
            let mut acc = 0.0f64;
            let mut dim = dims;
            for d in 0..dims {
                let e = f64::from(hi[d].max(x[d]) - lo[d].min(x[d]));
                if e > 0.0 {
                    dim = d;
                    acc += e;
                    if acc > pick {
                        break;
                    }
                }
            }
            if dim == dims {
                return false;
            }
            let (lo2, hi2) = (lo[dim].min(x[dim]), hi[dim].max(x[dim]));
            let mut cut = lo2 + ((f64::from(hi2 - lo2)) * rng.open_unit()) as f32;
            if cut >= hi2 {
                cut = hi2.next_down();
            }
            if cut < lo2 {
                cut = lo2;
            }
            let below = x[dim] < lo[dim] && cut < lo[dim];
            let above = x[dim] > hi[dim] && cut >= hi[dim];
            if below || above {
                return self.split(n, p, dim as u16, cut, below);
            }
            let node = self.nodes[n as usize];
            n = if x[usize::from(node.cut_dim)] <= node.cut_val {
                node.left
            } else {
                node.right
            };
        }
    }

    /// Puts a new inner node above `n` with `p` as its new sibling.
    fn split(&mut self, n: u32, p: u32, dim: u16, cut: f32, x_is_left: bool) -> bool {
        if self.free.len() < 2 {
            return false;
        }
        let old = self.nodes[n as usize];
        let leaf = self
            .alloc(Node {
                mass: 1,
                point: p,
                ..Node::EMPTY
            })
            .expect("two nodes are free");
        let (left, right) = if x_is_left { (leaf, n) } else { (n, leaf) };
        let inner = self
            .alloc(Node {
                parent: old.parent,
                left,
                right,
                mass: old.mass + 1,
                point: NIL,
                cut_val: cut,
                cut_dim: dim,
            })
            .expect("two nodes are free");
        self.nodes[leaf as usize].parent = inner;
        self.nodes[n as usize].parent = inner;
        if old.parent == NIL {
            self.root = inner;
        } else {
            let parent = &mut self.nodes[old.parent as usize];
            if parent.left == n {
                parent.left = inner;
            } else {
                parent.right = inner;
            }
        }
        self.add_mass_up(old.parent, 1);
        true
    }

    /// Removes one occurrence of the point stored at `p`.
    pub(crate) fn delete(&mut self, pts: &Points, p: u32) -> Deleted {
        let x = pts.get(p);
        let mut n = self.root;
        let mut steps = 0usize;
        while n != NIL && !self.nodes[n as usize].is_leaf() {
            let node = self.nodes[n as usize];
            n = if x[usize::from(node.cut_dim)] <= node.cut_val {
                node.left
            } else {
                node.right
            };
            steps += 1;
            if steps > self.nodes.len() {
                return Deleted::Missing;
            }
        }
        if n == NIL || pts.get(self.nodes[n as usize].point) != x {
            return Deleted::Missing;
        }
        let leaf = self.nodes[n as usize];
        if leaf.mass > 1 {
            self.nodes[n as usize].mass -= 1;
            self.add_mass_up(leaf.parent, -1);
            return if leaf.point == p {
                Deleted::Repoint(n)
            } else {
                Deleted::Gone
            };
        }
        let parent = leaf.parent;
        self.release(n);
        if parent == NIL {
            self.root = NIL;
            return Deleted::Gone;
        }
        let pnode = self.nodes[parent as usize];
        let sibling = if pnode.left == n {
            pnode.right
        } else {
            pnode.left
        };
        self.nodes[sibling as usize].parent = pnode.parent;
        if pnode.parent == NIL {
            self.root = sibling;
        } else {
            let g = &mut self.nodes[pnode.parent as usize];
            if g.left == parent {
                g.left = sibling;
            } else {
                g.right = sibling;
            }
        }
        self.release(parent);
        self.add_mass_up(pnode.parent, -1);
        Deleted::Gone
    }

    /// Names another stored point for a leaf whose point is being retired.
    pub(crate) fn repoint(&mut self, leaf: u32, point: u32) {
        self.nodes[leaf as usize].point = point;
    }

    /// The anomaly score of `x` in this tree: how likely `x` is separated from
    /// the points under each node it passes, weighted by how shallow that is.
    pub(crate) fn score(
        &self,
        pts: &Points,
        ws: &mut Workspace,
        x: &[f32],
        sample_size: usize,
    ) -> f64 {
        if self.root == NIL {
            return 0.0;
        }
        self.boxes(pts, ws);
        let dims = pts.dims;
        let mut n = self.root;
        let mut depth = 0.0f64;
        let mut remaining = 1.0f64;
        let mut acc = 0.0f64;
        loop {
            let node = self.nodes[n as usize];
            let at = n as usize * dims;
            let (lo, hi) = (&ws.lo[at..at + dims], &ws.hi[at..at + dims]);
            let (mut outside, mut extent) = (0.0f64, 0.0f64);
            for d in 0..dims {
                outside += f64::from((lo[d] - x[d]).max(0.0) + (x[d] - hi[d]).max(0.0));
                extent += f64::from(hi[d] - lo[d]);
            }
            let sep = if outside == 0.0 {
                0.0
            } else {
                outside / (outside + extent)
            };
            let base = depth + (f64::from(node.mass) + 1.0).log2();
            acc += remaining * sep / (depth + 1.0);
            remaining *= 1.0 - sep;
            if node.is_leaf() {
                let damp = 1.0 - f64::from(node.mass) / (2.0 * sample_size as f64);
                acc += remaining * damp / base;
                return acc;
            }
            n = if x[usize::from(node.cut_dim)] <= node.cut_val {
                node.left
            } else {
                node.right
            };
            depth += 1.0;
        }
    }

    /// The leaf that holds the point stored at `p`, found by descending on its values.
    pub(crate) fn leaf_of(&self, pts: &Points, p: u32) -> Option<u32> {
        let x = pts.get(p);
        let mut n = self.root;
        for _ in 0..=self.nodes.len() {
            if n == NIL {
                return None;
            }
            let node = &self.nodes[n as usize];
            if node.is_leaf() {
                return (pts.get(node.point) == x).then_some(n);
            }
            n = if x[usize::from(node.cut_dim)] <= node.cut_val {
                node.left
            } else {
                node.right
            };
        }
        None
    }

    /// Checks the arena is a well-formed tree over `store` stored points of
    /// `dims` values, returning the total mass. Used on a restored
    /// checkpoint, which is untrusted.
    pub(crate) fn verify(&self, store: usize, dims: usize) -> Option<u32> {
        let cap = self.nodes.len();
        let mut seen = vec![false; cap];
        let mut visited = 0usize;
        let mut mass_total = 0u32;
        if self.root == NIL {
            return (self.free.len() == cap && self.free_ok(&seen)).then_some(0);
        }
        if self.root as usize >= cap || self.nodes[self.root as usize].parent != NIL {
            return None;
        }
        let mut stack = vec![self.root];
        while let Some(n) = stack.pop() {
            let i = n as usize;
            if i >= cap || seen[i] {
                return None;
            }
            seen[i] = true;
            visited += 1;
            let node = &self.nodes[i];
            if node.is_leaf() {
                if node.point as usize >= store
                    || node.mass == 0
                    || node.left != NIL
                    || node.right != NIL
                {
                    return None;
                }
                mass_total = mass_total.checked_add(node.mass)?;
            } else {
                let (l, r) = (node.left as usize, node.right as usize);
                if l >= cap || r >= cap || usize::from(node.cut_dim) >= dims {
                    return None;
                }
                if self.nodes[l].parent != n || self.nodes[r].parent != n {
                    return None;
                }
                if self.nodes[l].mass.checked_add(self.nodes[r].mass)? != node.mass {
                    return None;
                }
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        (visited + self.free.len() == cap && self.free_ok(&seen)).then_some(mass_total)
    }

    fn free_ok(&self, seen: &[bool]) -> bool {
        let mut listed = vec![false; seen.len()];
        for &f in &self.free {
            let i = f as usize;
            if i >= seen.len() || seen[i] || listed[i] || self.nodes[i] != Node::EMPTY {
                return false;
            }
            listed[i] = true;
        }
        true
    }
}
