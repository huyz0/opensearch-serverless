//! The point catalogue and the tiers
//! ([ADR 0080](../../../docs/adr/0080-one-plugin-system-three-tiers.md),
//! section 3): which points exist, how deterministic each is, how large its
//! blocks may be, and on which tier each may run.

/// Where a plugin runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// A trusted plugin: a shared library the node loads at start, or code linked in.
    Native,
    /// A sandboxed WebAssembly module.
    Wasm,
    /// A separate process reached over gRPC.
    Grpc,
}

impl Tier {
    /// The name a manifest, a policy and a log line use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Wasm => "wasm",
            Self::Grpc => "grpc",
        }
    }

    /// The tier a name stands for.
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Native, Self::Wasm, Self::Grpc]
            .into_iter()
            .find(|t| t.name() == name)
    }
}

/// Whether a point's output depends on its input alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Determinism {
    /// A function of the input: no host import, no clock, replayable.
    Pure,
    /// May use granted capabilities and the clock; never replayed.
    Effectful,
}

/// One point of the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointSpec {
    /// The point's name.
    pub name: &'static str,
    /// The version this host serves; a manifest must name it exactly.
    pub version: u32,
    /// Whether the point is pure.
    pub determinism: Determinism,
    /// The tiers that may serve it.
    pub tiers: &'static [Tier],
    /// The most bytes of input one call carries; a point's own spec may lower it.
    pub max_in: u32,
    /// The most bytes of output one call returns; a point's own spec may lower it.
    pub max_out: u32,
}

impl PointSpec {
    /// Whether `tier` may serve this point.
    pub fn served_on(&self, tier: Tier) -> bool {
        self.tiers.contains(&tier)
    }
}

const ALL: &[Tier] = &[Tier::Native, Tier::Wasm, Tier::Grpc];
const IN_PROCESS: &[Tier] = &[Tier::Native, Tier::Wasm];
const NATIVE: &[Tier] = &[Tier::Native];
const MIB: u32 = 1024 * 1024;

/// Every point, as ADR 0080's catalogue lists them.
pub static POINTS: &[PointSpec] = &[
    PointSpec {
        name: "rest",
        version: 1,
        determinism: Determinism::Effectful,
        tiers: ALL,
        max_in: MIB,
        max_out: 4 * MIB,
    },
    PointSpec {
        name: "lifecycle",
        version: 1,
        determinism: Determinism::Effectful,
        tiers: ALL,
        max_in: 16 * 1024,
        max_out: 0,
    },
    PointSpec {
        name: "analyzer",
        version: 1,
        determinism: Determinism::Pure,
        tiers: IN_PROCESS,
        max_in: MIB,
        max_out: 4 * MIB,
    },
    PointSpec {
        name: "ingest",
        version: 1,
        determinism: Determinism::Effectful,
        tiers: ALL,
        max_in: 4 * MIB,
        max_out: 4 * MIB,
    },
    PointSpec {
        name: "query",
        version: 1,
        determinism: Determinism::Effectful,
        tiers: ALL,
        max_in: 64 * 1024,
        max_out: 256 * 1024,
    },
    PointSpec {
        name: "pipeline_agg",
        version: 1,
        determinism: Determinism::Effectful,
        tiers: ALL,
        max_in: 4 * MIB,
        max_out: 4 * MIB,
    },
    PointSpec {
        name: "script",
        version: 1,
        determinism: Determinism::Pure,
        tiers: IN_PROCESS,
        max_in: MIB,
        max_out: MIB,
    },
    PointSpec {
        name: "agg",
        version: 1,
        determinism: Determinism::Effectful,
        tiers: NATIVE,
        max_in: 4 * MIB,
        max_out: 4 * MIB,
    },
];

/// The point called `name`.
pub fn point(name: &str) -> Option<&'static PointSpec> {
    POINTS.iter().find(|p| p.name == name)
}
