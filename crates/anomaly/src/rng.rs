//! A seeded xoshiro256++ generator: the model's only source of randomness.

/// A deterministic generator; its four words are part of a checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// A generator whose state is expanded from `seed` by splitmix64.
    pub(crate) fn new(seed: u64) -> Self {
        let mut x = seed;
        let mut s = [0u64; 4];
        for word in &mut s {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            *word = z ^ (z >> 31);
        }
        Self { s }
    }

    /// The generator at a state read from a checkpoint; refused when all words are zero.
    pub(crate) fn from_state(s: [u64; 4]) -> Option<Self> {
        (s != [0; 4]).then_some(Self { s })
    }

    /// The four state words.
    pub(crate) fn state(&self) -> [u64; 4] {
        self.s
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let result = self.s[0]
            .wrapping_add(self.s[3])
            .rotate_left(23)
            .wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// A uniform value in the open interval (0, 1).
    pub(crate) fn open_unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
}
