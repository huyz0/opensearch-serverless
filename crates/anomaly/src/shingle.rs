//! Shingling: a point is the last `shingle_size` windows, oldest first.

// ARITH: `filled` and `head` are bounded by `shingle_size` (<= 60) and the
// feature count (<= 64).
#![allow(clippy::arithmetic_side_effects)]

use crate::params::{Imputation, Params};

/// A ring of the last windows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shingle {
    pub(crate) windows: Vec<f32>,
    pub(crate) features: usize,
    pub(crate) size: usize,
    /// Windows held, up to `size`.
    pub(crate) filled: usize,
    /// The ring slot the next window goes to.
    pub(crate) head: usize,
    /// The last window pushed, for `Previous` imputation.
    pub(crate) last: Vec<f32>,
    pub(crate) has_last: bool,
}

impl Shingle {
    pub(crate) fn new(p: &Params) -> Self {
        Self {
            windows: vec![0.0; p.dims()],
            features: p.features,
            size: p.shingle_size,
            filled: 0,
            head: 0,
            last: vec![0.0; p.features],
            has_last: false,
        }
    }

    /// Forgets every window (a gap too long to impute).
    pub(crate) fn clear(&mut self) {
        self.filled = 0;
        self.head = 0;
    }

    pub(crate) fn push(&mut self, window: &[f32]) {
        let at = self.head * self.features;
        self.windows[at..at + self.features].copy_from_slice(window);
        self.head = (self.head + 1) % self.size;
        self.filled = (self.filled + 1).min(self.size);
        self.last.copy_from_slice(window);
        self.has_last = true;
    }

    pub(crate) fn full(&self) -> bool {
        self.filled == self.size
    }

    /// Writes the point, oldest window first, into `out`.
    pub(crate) fn point(&self, out: &mut [f32]) {
        for i in 0..self.size {
            let ring = (self.head + i) % self.size;
            let from = ring * self.features;
            let to = i * self.features;
            out[to..to + self.features].copy_from_slice(&self.windows[from..from + self.features]);
        }
    }

    /// The window that stands in for a missing one.
    pub(crate) fn imputed(&self, how: &Imputation, out: &mut [f32]) {
        match how {
            Imputation::Zero => out.fill(0.0),
            Imputation::Fixed(v) => out.copy_from_slice(v),
            Imputation::Previous => {
                if self.has_last {
                    out.copy_from_slice(&self.last);
                } else {
                    out.fill(0.0);
                }
            }
        }
    }
}
