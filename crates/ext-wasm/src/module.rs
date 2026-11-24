//! A loaded module and one call into it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oss_ext::{
    CallError, Descriptor, MAX_DESCRIPTOR_BYTES, Status, abi_accepted, pack_abi, printable,
    status_of,
};
use wasmtime::{
    Caller, Extern, Instance, InstancePre, Linker, Memory, Store, StoreLimits, StoreLimitsBuilder,
    Trap,
};

use crate::config::LogSink;
use crate::gate::Gate;
use crate::host::{LoadError, LogRate, Shared, TICK};
use crate::reserve::{Reservation, Reserver};

const MAX_LOG_BYTES: usize = 512;
const MAX_LOGS_PER_SECOND: u32 = 10;
/// How much of the output region a failure's detail reads.
const DETAIL_BYTES: u32 = 256;

struct State {
    limits: StoreLimits,
    name: Arc<str>,
    log: LogSink,
    rate: LogRate,
    started: Instant,
    deadline: Duration,
}

/// The limits one call runs under: the host's, lowered by the descriptor's.
#[derive(Clone, Copy)]
struct Effective {
    fuel: u64,
    memory: u64,
    deadline_ms: u64,
}

/// A compiled module with its descriptor, ready to be called.
pub struct Module {
    shared: Arc<Shared>,
    pre: InstancePre<State>,
    name: Arc<str>,
    descriptor: Descriptor,
    effective: Effective,
    gate: Gate,
    rate: LogRate,
    reserver: Arc<dyn Reserver>,
    in_ptr: u32,
    out_ptr: u32,
    _code: Reservation,
}

struct Session {
    store: Store<State>,
    instance: Instance,
    memory: Memory,
}

fn word(x: i32) -> u32 {
    u32::from_ne_bytes(x.to_ne_bytes())
}

fn within(ptr: u32, len: u32, memory: usize) -> bool {
    u64::from(ptr).saturating_add(u64::from(len)) <= memory as u64
}

/// A runtime failure as the typed failure of a call: a limit names itself,
/// any other trap is a trap.
fn failed(e: &wasmtime::Error) -> CallError {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => CallError::from_status(Status::Limit, b"fuel exhausted"),
        Some(Trap::Interrupt) => CallError::from_status(Status::Limit, b"deadline passed"),
        Some(Trap::StackOverflow) => CallError::from_status(Status::Limit, b"stack exhausted"),
        Some(t) => CallError::trap(&t.to_string()),
        None => CallError::from_status(Status::Limit, e.to_string().as_bytes()),
    }
}

fn linker(shared: &Shared) -> Result<Linker<State>, wasmtime::Error> {
    let mut l = Linker::new(&shared.engine);
    l.func_wrap(
        "oss",
        "log",
        |mut c: Caller<'_, State>, ptr: i32, len: i32| {
            let Some(Extern::Memory(m)) = c.get_export("memory") else {
                return;
            };
            let (ptr, len) = (word(ptr), word(len).min(MAX_LOG_BYTES as u32));
            let mut buf = vec![0_u8; len as usize]; // ALLOC: at most MAX_LOG_BYTES.
            if m.read(&c, ptr as usize, &mut buf).is_err() {
                return;
            }
            let allowed = {
                let mut r = c
                    .data()
                    .rate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if r.0.elapsed() >= Duration::from_secs(1) {
                    *r = (Instant::now(), 0);
                }
                r.1 = r.1.saturating_add(1);
                r.1 <= MAX_LOGS_PER_SECOND
            };
            if allowed {
                let d = c.data();
                (d.log)(&d.name, &printable(&buf, MAX_LOG_BYTES));
            }
        },
    )?;
    l.func_wrap("oss", "cancelled", |c: Caller<'_, State>| -> i32 {
        i32::from(
            c.data().started.elapsed() >= c.data().deadline.checked_div(2).unwrap_or_default(),
        )
    })?;
    Ok(l)
}

impl Module {
    pub(crate) fn compile(
        shared: Arc<Shared>,
        bytes: &[u8],
        reserver: Arc<dyn Reserver>,
        code: Reservation,
    ) -> Result<Self, LoadError> {
        let broken = |e: wasmtime::Error| LoadError::Broken(e.to_string());
        let module = wasmtime::Module::new(&shared.engine, bytes)
            .map_err(|e| LoadError::Refused(e.to_string()))?;
        let pre = linker(&shared)
            .and_then(|l| l.instantiate_pre(&module))
            .map_err(broken)?;
        let rate: LogRate = Arc::new(Mutex::new((Instant::now(), 0)));
        let (descriptor, in_ptr, out_ptr) = describe(&shared, &pre, &rate)?;
        let host = &shared.config;
        let l = descriptor.limits();
        let effective = Effective {
            fuel: l.fuel.map_or(host.fuel, |v| v.min(host.fuel)),
            memory: l
                .memory_bytes
                .map_or(host.memory_bytes, |v| v.min(host.memory_bytes)),
            deadline_ms: l
                .deadline_ms
                .map_or(host.deadline_ms, |v| v.min(host.deadline_ms)),
        };
        let gate = Gate::new(
            descriptor.max_concurrent_calls().min(host.concurrency),
            host.tenant_concurrency,
        );
        Ok(Self {
            shared,
            pre,
            name: Arc::from(descriptor.name()),
            descriptor,
            effective,
            gate,
            rate,
            reserver,
            in_ptr,
            out_ptr,
            _code: code,
        })
    }

    fn open(&self, eff: Effective) -> Result<Session, CallError> {
        open(&self.shared, &self.pre, &self.rate, eff, &self.name)
    }

    /// The module's descriptor.
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    /// Bytes one call reserves before it instantiates: linear memory, both
    /// I/O regions and the stack.
    pub fn call_reservation(&self) -> u64 {
        self.effective
            .memory
            .saturating_add(u64::from(self.descriptor.io_bytes()).saturating_mul(2))
            .saturating_add(self.shared.config.stack_bytes as u64)
    }

    /// Runs `input` through point number `point` of the descriptor for
    /// `tenant` on this thread, and returns the module's output after
    /// validating it.
    ///
    /// # Errors
    ///
    /// [`CallError`]: busy when the gate or pool is full, a limit or trap
    /// when the module exceeded one, malformed when it broke the contract.
    pub fn call(&self, tenant: &str, point: usize, input: &[u8]) -> Result<Vec<u8>, CallError> {
        let io = self.descriptor.io_bytes();
        if point >= self.descriptor.points().len() {
            return Err(CallError::from_status(Status::Unsupported, b""));
        }
        if input.len() > io as usize {
            return Err(CallError::from_status(
                Status::Limit,
                b"input larger than the region",
            ));
        }
        let _permit = self.gate.enter(tenant).ok_or_else(CallError::busy)?;
        let _hold = self
            .reserver
            .reserve(self.call_reservation())
            .ok_or_else(CallError::busy)?;
        let mut s = self.open(self.effective)?;
        s.memory
            .write(&mut s.store, self.in_ptr as usize, input)
            .map_err(|_| CallError::malformed_because("the input region is outside memory"))?;
        let f = s
            .instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut s.store, "oss_call")
            .map_err(|_| CallError::malformed_because("no oss_call"))?;
        let len = |s: &mut Session| -> Result<u32, CallError> {
            s.instance
                .get_typed_func::<(), i32>(&mut s.store, "oss_result_len")
                .and_then(|g| g.call(&mut s.store, ()))
                .map(word)
                .map_err(|e| failed(&e))
        };
        let point = i32::try_from(point).unwrap_or(i32::MAX);
        let in_len = i32::try_from(input.len()).unwrap_or(i32::MAX);
        let cap = i32::try_from(io).unwrap_or(i32::MAX);
        let code = f
            .call(&mut s.store, (point, in_len, cap))
            .map_err(|e| failed(&e))?;
        let status = status_of(code).map_err(CallError::malformed)?;
        let n = len(&mut s)?;
        let mem = s.memory.data_size(&s.store);
        let read = |s: &Session, n: u32| -> Result<Vec<u8>, CallError> {
            // ALLOC: at most the region the descriptor declared, reserved above.
            let mut out = vec![0_u8; n as usize];
            s.memory
                .read(&s.store, self.out_ptr as usize, &mut out)
                .map_err(|_| CallError::malformed_because("the output is outside memory"))?;
            Ok(out)
        };
        if status != Status::Ok {
            let n = n.min(DETAIL_BYTES);
            let detail = if within(self.out_ptr, n, mem) {
                read(&s, n)?
            } else {
                Vec::new()
            };
            return Err(CallError::from_status(status, &detail));
        }
        if n > io || !within(self.out_ptr, n, mem) {
            return Err(CallError::malformed_because(
                "output length past its region",
            ));
        }
        read(&s, n)
    }
}

fn open(
    shared: &Shared,
    pre: &InstancePre<State>,
    rate: &LogRate,
    eff: Effective,
    name: &Arc<str>,
) -> Result<Session, CallError> {
    let deadline = Duration::from_millis(eff.deadline_ms);
    let state = State {
        limits: StoreLimitsBuilder::new()
            .memory_size(usize::try_from(eff.memory).unwrap_or(usize::MAX))
            .table_elements(10_000)
            .instances(1)
            .trap_on_grow_failure(false)
            .build(),
        name: Arc::clone(name),
        log: Arc::clone(&shared.config.log),
        rate: Arc::clone(rate),
        started: Instant::now(),
        deadline,
    };
    let mut store = Store::new(&shared.engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_fuel(eff.fuel).map_err(|e| failed(&e))?;
    let ticks = deadline.as_millis().div_ceil(TICK.as_millis()).max(1);
    store.set_epoch_deadline(u64::try_from(ticks).unwrap_or(u64::MAX));
    let instance = pre.instantiate(&mut store).map_err(|e| failed(&e))?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| CallError::malformed_because("no exported memory"))?;
    Ok(Session {
        store,
        instance,
        memory,
    })
}

fn word_of(s: &mut Session, export: &str) -> Result<u32, LoadError> {
    s.instance
        .get_typed_func::<(), i32>(&mut s.store, export)
        .and_then(|f| f.call(&mut s.store, ()))
        .map(word)
        .map_err(|e| LoadError::Broken(format!("{export}: {}", failed(&e).detail())))
}

/// Reads the module's version, descriptor and regions with the host's
/// limits.
fn describe(
    shared: &Shared,
    pre: &InstancePre<State>,
    rate: &LogRate,
) -> Result<(Descriptor, u32, u32), LoadError> {
    let broken = |why: &str| LoadError::Broken(why.to_owned());
    let eff = Effective {
        fuel: shared.config.fuel,
        memory: shared.config.memory_bytes,
        deadline_ms: shared.config.deadline_ms,
    };
    let name: Arc<str> = Arc::from("(describing)");
    let mut s = open(shared, pre, rate, eff, &name)
        .map_err(|e| LoadError::Broken(e.detail().to_owned()))?;
    abi_accepted(word_of(&mut s, "oss_abi_version")?).map_err(LoadError::Abi)?;
    let len = word_of(&mut s, "oss_describe")?;
    let out = word_of(&mut s, "oss_output_region")?;
    if len > MAX_DESCRIPTOR_BYTES || !within(out, len, s.memory.data_size(&s.store)) {
        return Err(broken("the descriptor is outside the output region"));
    }
    // ALLOC: at most MAX_DESCRIPTOR_BYTES.
    let mut raw = vec![0_u8; len as usize];
    s.memory
        .read(&s.store, out as usize, &mut raw)
        .map_err(|_| broken("the descriptor is unreadable"))?;
    let d = Descriptor::parse(&raw).map_err(LoadError::Descriptor)?;
    let (major, minor) = d.abi();
    abi_accepted(pack_abi(major, minor)).map_err(LoadError::Abi)?;
    let input = word_of(&mut s, "oss_input_region")?;
    let io = d.io_bytes();
    let mem = s.memory.data_size(&s.store);
    let disjoint = input.saturating_add(io) <= out || out.saturating_add(io) <= input;
    if !within(input, io, mem) || !within(out, io, mem) || !disjoint {
        return Err(broken(
            "an I/O region is outside memory or overlaps the other",
        ));
    }
    Ok((d, input, out))
}
