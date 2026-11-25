//! The engine, its deadline ticker, and loading a module.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_ext::{DescriptorError, MAX_MODULE_BYTES, VersionError};
use wasmtime::{Config, Engine};

use crate::config::HostConfig;
use crate::module::Module;
use crate::reserve::Reserver;
use crate::validate;

/// How often the epoch advances; a deadline is a whole number of these.
pub(crate) const TICK: Duration = Duration::from_millis(10);

/// Compile memory is reserved at this multiple of the module's size.
const COMPILE_FACTOR: u64 = 16;

/// What the engine and its ticker share with every module.
pub(crate) struct Shared {
    pub(crate) engine: Engine,
    pub(crate) config: HostConfig,
    stop: Arc<AtomicBool>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Why a module was not loaded.
#[derive(Debug)]
pub enum LoadError {
    /// Larger than [`MAX_MODULE_BYTES`].
    TooLarge,
    /// Refused before compiling, or not a valid module.
    Refused(String),
    /// Built for an ABI this host does not accept.
    Abi(VersionError),
    /// The descriptor was refused.
    Descriptor(DescriptorError),
    /// The module does not follow the contract.
    Broken(String),
    /// The pool could not hold the compile.
    Busy,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(f, "module larger than {MAX_MODULE_BYTES} bytes"),
            Self::Refused(why) | Self::Broken(why) => f.write_str(why),
            Self::Abi(e) => write!(f, "ABI version refused: {e:?}"),
            Self::Descriptor(e) => e.fmt(f),
            Self::Busy => f.write_str("no capacity to compile the module"),
        }
    }
}

impl std::error::Error for LoadError {}

/// The WASM host: one engine for every module. A clone is the same host.
#[derive(Clone)]
pub struct Host {
    shared: Arc<Shared>,
}

impl Host {
    /// A host with `config`'s limits. Threads, relaxed SIMD, multiple
    /// memories and a start function are refused and NaNs are canonicalised,
    /// so a module behaves the same on every node.
    ///
    /// # Panics
    ///
    /// When the engine cannot be built with this fixed configuration, which
    /// is a build defect and not an input.
    pub fn new(config: HostConfig) -> Self {
        let mut c = Config::new();
        c.consume_fuel(true)
            .epoch_interruption(true)
            .wasm_relaxed_simd(false)
            .wasm_multi_memory(false)
            .wasm_memory64(false)
            .cranelift_nan_canonicalization(true)
            .max_wasm_stack(config.stack_bytes);
        let engine = Engine::new(&c).expect("the fixed engine configuration is valid");
        let stop = Arc::new(AtomicBool::new(false));
        let (e, s) = (engine.clone(), Arc::clone(&stop));
        // ALLOC: one ticker thread per host.
        std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                std::thread::sleep(TICK);
                e.increment_epoch();
            }
        });
        Self {
            shared: Arc::new(Shared {
                engine,
                config,
                stop,
            }),
        }
    }

    /// Validates, compiles and describes the module `bytes`.
    ///
    /// # Errors
    ///
    /// [`LoadError`] naming why the module is not usable.
    pub fn load(&self, bytes: &[u8], reserver: Arc<dyn Reserver>) -> Result<Module, LoadError> {
        if bytes.len() > MAX_MODULE_BYTES as usize {
            return Err(LoadError::TooLarge);
        }
        if let Some(why) = validate::refuse(bytes) {
            return Err(LoadError::Refused(why));
        }
        let charge = (bytes.len() as u64).saturating_mul(COMPILE_FACTOR);
        let hold = reserver.reserve(charge).ok_or(LoadError::Busy)?;
        // Describing runs the module with the host's memory and stack, so
        // that is held too until it is described.
        let c = &self.shared.config;
        let _describing = reserver
            .reserve(c.memory_bytes.saturating_add(c.stack_bytes as u64))
            .ok_or(LoadError::Busy)?;
        Module::compile(Arc::clone(&self.shared), bytes, reserver, hold)
    }
}

/// A rate window shared by the calls of one module.
pub(crate) type LogRate = Arc<Mutex<(std::time::Instant, u32)>>;
