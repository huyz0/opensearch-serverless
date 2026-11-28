//! Scratch memory one worker lends to every model it runs.

use crate::params::Params;

/// Bounding boxes of one tree's nodes and the order they are computed in. A
/// worker owns one, sized for the largest model it will run, and lends it to
/// each call; a model holds none of it.
#[derive(Debug)]
pub struct Workspace {
    pub(crate) lo: Vec<f32>,
    pub(crate) hi: Vec<f32>,
    pub(crate) order: Vec<u32>,
    pub(crate) stack: Vec<u32>,
    pub(crate) dims: usize,
    pub(crate) nodes: usize,
}

impl Workspace {
    /// A workspace for models of `params`' shape (or smaller).
    pub fn new(params: &Params) -> Self {
        let nodes = params.node_capacity();
        let dims = params.dims();
        // ARITH: nodes <= 4,095 and dims <= 512, so the product fits a usize.
        #[allow(clippy::arithmetic_side_effects)]
        let cells = nodes * dims;
        Self {
            lo: vec![0.0; cells],
            hi: vec![0.0; cells],
            order: Vec::with_capacity(nodes),
            stack: Vec::with_capacity(nodes),
            dims,
            nodes,
        }
    }

    /// Whether a model of `params` fits in this workspace.
    pub fn fits(&self, params: &Params) -> bool {
        params.dims() <= self.dims && params.node_capacity() <= self.nodes
    }
}
