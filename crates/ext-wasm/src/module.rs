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
/// Fuel one `log` call costs, whether or not the line is kept.
const LOG_FUEL: u64 = 1_000;
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

/// How long a replay sleeps between tries for its reservation.
const RESERVE_POLL: Duration = Duration::from_millis(25);

/// The limits one call runs under: the host's, lowered by the descriptor's.
#[derive(Clone, Copy)]
struct Effective {
    fuel: u64,
    memory: u64,
    deadline_ms: u64,
}

/// A replay of a logged write: the limits its index recorded, stretched.
#[derive(Clone, Copy, Debug)]
pub struct Replay {
    /// What the index recorded when its pin was made.
    pub limits: oss_ext::pin::ResolvedLimits,
    /// Fuel and memory are the record times this; the deadline is ten
    /// times the record, a guard against a hang and not a correctness limit.
    pub factor: u64,
    /// How long a call waits for its reservation before it answers busy.
    pub wait: Duration,
}

/// How a call is run.
#[derive(Clone, Copy, Debug)]
pub enum Run {
    /// Under the module's own limits, behind the gate.
    Live,
    /// Under the limits an index recorded exactly, behind the gate.
    Recorded(oss_ext::pin::ResolvedLimits),
    /// A replay: never refused by the gate.
    Replay(Replay),
}

impl Run {
    fn effective(self, own: Effective) -> Effective {
        match self {
            Run::Live => own,
            Run::Recorded(l) => Effective {
                fuel: l.fuel,
                memory: l.memory_bytes,
                deadline_ms: l.deadline_ms,
            },
            Run::Replay(r) => Effective {
                fuel: r.limits.fuel.saturating_mul(r.factor),
                memory: r.limits.memory_bytes.saturating_mul(r.factor),
                deadline_ms: r.limits.deadline_ms.saturating_mul(10),
            },
        }
    }
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
    imports: usize,
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
            // A refused line costs the module fuel and the host no copy.
            let left = c.get_fuel().unwrap_or(0);
            let _ = c.set_fuel(left.saturating_sub(LOG_FUEL));
            let Some(Extern::Memory(m)) = c.get_export("memory") else {
                return;
            };
            if !allowed {
                return;
            }
            let (ptr, len) = (word(ptr), word(len).min(MAX_LOG_BYTES as u32));
            let mut buf = vec![0_u8; len as usize]; // ALLOC: at most MAX_LOG_BYTES.
            if m.read(&c, ptr as usize, &mut buf).is_err() {
                return;
            }
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
            imports: crate::validate::import_count(bytes),
            _code: code,
        })
    }

    fn open(&self, eff: Effective) -> Result<Session, CallError> {
        open(&self.shared, &self.pre, &self.rate, eff, &self.name)
    }

    /// How many imports the module declares.
    pub(crate) fn import_count(&self) -> usize {
        self.imports
    }

    /// The limits a call of this module runs under: the host's, lowered by
    /// the descriptor's.
    pub fn resolved_limits(&self) -> oss_ext::pin::ResolvedLimits {
        oss_ext::pin::ResolvedLimits {
            fuel: self.effective.fuel,
            memory_bytes: self.effective.memory,
            deadline_ms: self.effective.deadline_ms,
        }
    }

    /// The module's descriptor.
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    /// Bytes one call reserves before it instantiates: linear memory, both
    /// I/O regions and the stack.
    pub fn call_reservation(&self) -> u64 {
        self.reservation_of(self.effective)
    }

    fn reservation_of(&self, eff: Effective) -> u64 {
        eff.memory
            .saturating_add(u64::from(self.descriptor.io_bytes()).saturating_mul(2))
            .saturating_add(self.shared.config.stack_bytes as u64)
    }

    /// The reservation of a call under `eff`, waiting up to `wait` for the
    /// pool to hold it.
    fn reserve_within(&self, eff: Effective, wait: Duration) -> Result<Reservation, CallError> {
        let bytes = self.reservation_of(eff);
        let started = Instant::now();
        loop {
            if let Some(hold) = self.reserver.reserve(bytes) {
                return Ok(hold);
            }
            if started.elapsed() >= wait {
                return Err(CallError::busy());
            }
            std::thread::sleep(RESERVE_POLL);
        }
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
        self.call_as(tenant, point, input, Run::Live)
    }

    /// As [`Module::call`], under the limits and admission `run` names.
    ///
    /// # Errors
    ///
    /// As [`Module::call`]; a replay is busy only when the pool still cannot
    /// hold the call after its wait.
    pub fn call_as(
        &self,
        tenant: &str,
        point: usize,
        input: &[u8],
        run: Run,
    ) -> Result<Vec<u8>, CallError> {
        let mut spent = 0;
        let out = self.run_call(tenant, point, input, run, &mut spent);
        // A call refused before it ran (the gate, the pool, the input) costs nothing.
        if spent > 0 || out.is_ok() {
            let sent = out.as_ref().map_or(0, Vec::len);
            self.shared.meter.record(tenant, spent, input.len(), sent);
        }
        out
    }

    fn run_call(
        &self,
        tenant: &str,
        point: usize,
        input: &[u8],
        run: Run,
        spent: &mut u64,
    ) -> Result<Vec<u8>, CallError> {
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
        let eff = run.effective(self.effective);
        let (_permit, _hold) = match run {
            Run::Replay(replay) => (None, self.reserve_within(eff, replay.wait)?),
            Run::Live | Run::Recorded(_) => {
                let permit = self.gate.enter(tenant).ok_or_else(CallError::busy)?;
                let hold = self
                    .reserver
                    .reserve(self.reservation_of(eff))
                    .ok_or_else(CallError::busy)?;
                (Some(permit), hold)
            }
        };
        let mut s = self.open(eff)?;
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
        let called = f.call(&mut s.store, (point, in_len, cap));
        *spent = eff
            .fuel
            .saturating_sub(s.store.get_fuel().unwrap_or(eff.fuel));
        let code = called.map_err(|e| failed(&e))?;
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
            .tables(4)
            .memories(1)
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
