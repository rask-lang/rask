// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Runtime values.

use std::collections::HashMap;
use std::fmt;
use std::fs::File as StdFile;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};

use rask_ast::expr::Expr;

/// Global pool ID counter. Each Pool gets a unique ID.
static NEXT_POOL_ID: AtomicU32 = AtomicU32::new(1);

/// Allocate the next unique pool ID.
pub fn next_pool_id() -> u32 {
    NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed)
}

/// Internal pool storage. Sparse array with generation counters for handle validation.
#[derive(Debug, Clone)]
pub struct PoolData {
    pub pool_id: u32,
    /// Sparse storage: each slot is (generation, Option<Value>).
    pub slots: Vec<(u32, Option<Value>)>,
    /// Free slot indices available for reuse.
    pub free_list: Vec<u32>,
    /// Count of live elements.
    pub len: usize,
    /// Type parameter for generic Pool<T> (e.g., "Node" in Pool<Node>).
    pub type_param: Option<String>,
}

impl PoolData {
    pub fn new() -> Self {
        Self {
            pool_id: next_pool_id(),
            slots: Vec::new(),
            free_list: Vec::new(),
            len: 0,
            type_param: None,
        }
    }

    pub fn with_type_param(type_param: Option<String>) -> Self {
        Self {
            pool_id: next_pool_id(),
            slots: Vec::new(),
            free_list: Vec::new(),
            len: 0,
            type_param,
        }
    }

    /// Validate a handle against this pool. Returns the slot index on success.
    pub fn validate(&self, pool_id: u32, index: u32, generation: u32) -> Result<usize, String> {
        if pool_id != self.pool_id {
            return Err("handle from wrong pool".to_string());
        }
        let idx = index as usize;
        if idx >= self.slots.len() {
            return Err("invalid handle index".to_string());
        }
        let (slot_gen, ref slot_val) = self.slots[idx];
        if slot_gen != generation {
            return Err("stale handle".to_string());
        }
        if slot_val.is_none() {
            return Err("stale handle".to_string());
        }
        Ok(idx)
    }

    /// Insert a value into the pool. Returns (index, generation) for the handle.
    pub fn insert(&mut self, value: Value) -> (u32, u32) {
        if let Some(free_idx) = self.free_list.pop() {
            let idx = free_idx as usize;
            let gen = self.slots[idx].0; // generation was already bumped on remove
            self.slots[idx].1 = Some(value);
            self.len += 1;
            (free_idx, gen)
        } else {
            let idx = self.slots.len() as u32;
            let gen = 1u32; // first generation for new slots
            self.slots.push((gen, Some(value)));
            self.len += 1;
            (idx, gen)
        }
    }

    /// Remove a value at the given validated index. Bumps generation for the slot.
    pub fn remove_at(&mut self, idx: usize) -> Option<Value> {
        let (ref mut gen, ref mut slot) = self.slots[idx];
        if let Some(val) = slot.take() {
            *gen = gen.saturating_add(1); // bump generation (saturating per spec)
            self.free_list.push(idx as u32);
            self.len -= 1;
            Some(val)
        } else {
            None
        }
    }

    /// Collect all valid (index, generation) pairs.
    pub fn valid_handles(&self) -> Vec<(u32, u32)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, (gen, slot))| {
                if slot.is_some() {
                    Some((i as u32, *gen))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Built-in function kinds (global functions without module prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Print,
    Println,
    Panic,
    Format,
    AsyncSpawn, // spawn(|| {}) from async module
}

/// Type constructor kinds (for static method calls like Vec.new()).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeConstructorKind {
    Vec,
    Map,
    String,
    Pool,
    Channel,
    Shared,
    Atomic,
    Ordering,
}

/// Module kinds for stdlib modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Fs,     // fs.read_file, fs.write_file, etc.
    Io,     // io.read_line, io.print, etc.
    Cli,    // cli.parse, cli.Parser (also legacy cli.args)
    Std,    // std.exit (legacy alias for os.exit)
    Env,    // env.var, env.vars (legacy alias for os.env)
    Time,   // time.Instant, time.Duration, time.sleep
    Random, // random.f64, random.range, Rng, etc.
    Math,   // math.sin, math.PI, etc.
    Os,     // os.env, os.args, os.exit, os.platform, etc.
    Json,   // json.parse, json.stringify, json.encode, etc.
    Path,   // Path.new (type constructor via module)
    Net,    // net.tcp_listen, net.tcp_connect
    Async,  // async.spawn (green task spawner)
    Thread, // thread.Thread, thread.ThreadPool
}

/// Inner state for a spawned thread handle.
pub struct ThreadHandleInner {
    pub handle: Mutex<Option<std::thread::JoinHandle<Result<Value, String>>>>,
}

impl fmt::Debug for ThreadHandleInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThreadHandleInner")
    }
}

/// A task submitted to a thread pool.
pub struct PoolTask {
    pub work: Box<dyn FnOnce() + Send>,
}

/// Inner state for a thread pool.
pub struct ThreadPoolInner {
    pub sender: Mutex<Option<mpsc::Sender<PoolTask>>>,
    pub workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
    pub size: usize,
}

impl fmt::Debug for ThreadPoolInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThreadPoolInner(size={})", self.size)
    }
}

/// Multitasking runtime (in interpreter, just tracks that we're in async context).
pub struct MultitaskingRuntime {
    /// Number of worker threads (unused in interpreter, just for compatibility).
    pub workers: usize,
}

impl fmt::Debug for MultitaskingRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MultitaskingRuntime(workers={})", self.workers)
    }
}

/// A runtime value in the interpreter.
#[derive(Debug, Clone)]
pub enum Value {
    /// Unit value
    Unit,
    /// Boolean
    Bool(bool),
    /// Integer (using i64 for all integer types in interpreter)
    Int(i64),
    /// 128-bit signed integer
    Int128(i128),
    /// 128-bit unsigned integer
    Uint128(u128),
    /// Float (using f64 for all float types in interpreter)
    Float(f64),
    /// Character
    Char(char),
    /// String (mutable, like Vec)
    String(Arc<Mutex<String>>),
    /// Struct instance
    Struct {
        name: String,
        fields: HashMap<String, Value>,
        /// Resource tracking ID (Some for @resource types).
        resource_id: Option<u64>,
    },
    /// Enum variant
    Enum {
        name: String,
        variant: String,
        fields: Vec<Value>,
    },
    /// Function reference
    Function {
        name: String,
    },
    /// Built-in function
    Builtin(BuiltinKind),
    /// Range value (for iteration)
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    /// Vec (growable array) with interior mutability
    Vec(Arc<Mutex<Vec<Value>>>),
    /// Type constructor (for static method calls like Vec.new())
    TypeConstructor {
        kind: TypeConstructorKind,
        type_param: Option<String>,
    },
    /// Enum variant constructor (e.g., Option.Some before calling with args)
    EnumConstructor {
        enum_name: String,
        variant_name: String,
        field_count: usize,
    },
    /// Module (fs, io, cli, std, env)
    Module(ModuleKind),
    /// User package namespace (for cross-package qualified access)
    Package(String),
    /// Open file handle (Option allows close to invalidate)
    File(Arc<Mutex<Option<StdFile>>>),
    /// Closure (captured environment + params + body)
    Closure {
        params: Vec<String>,
        body: Expr,
        captured_env: HashMap<String, Value>,
    },
    /// Duration (time span in nanoseconds)
    Duration(u64),
    /// Instant (monotonic timestamp)
    Instant(std::time::Instant),
    /// Type value (for accessing static methods like Instant.now())
    Type(String),
    /// Pool (sparse storage with generation counters)
    Pool(Arc<Mutex<PoolData>>),
    /// Handle (opaque reference into a pool)
    Handle {
        pool_id: u32,
        index: u32,
        generation: u32,
    },
    /// Thread handle (from spawn_raw or spawn_thread)
    ThreadHandle(Arc<ThreadHandleInner>),
    /// Channel sender
    Sender(Arc<Mutex<mpsc::SyncSender<Value>>>),
    /// Channel receiver
    Receiver(Arc<Mutex<mpsc::Receiver<Value>>>),
    /// Thread pool (from `using ThreadPool(workers: n) { }`)
    ThreadPool(Arc<ThreadPoolInner>),
    /// Async task handle (from spawn() in using Multitasking)
    TaskHandle(Arc<ThreadHandleInner>),
    /// Multitasking runtime (from `using Multitasking { }`)
    MultitaskingRuntime(Arc<MultitaskingRuntime>),
    /// Map (key-value storage with Value keys)
    Map(Arc<Mutex<Vec<(Value, Value)>>>),
    /// Atomic bool (lock-free boolean)
    AtomicBool(Arc<std::sync::atomic::AtomicBool>),
    /// Atomic usize (lock-free unsigned integer)
    AtomicUsize(Arc<std::sync::atomic::AtomicUsize>),
    /// Atomic u64 (lock-free 64-bit unsigned integer)
    AtomicU64(Arc<std::sync::atomic::AtomicU64>),
    /// Shared<T> (RwLock wrapper for concurrent read-heavy access)
    Shared(Arc<RwLock<Value>>),
    /// TCP listener socket (Option allows close to invalidate)
    TcpListener(Arc<Mutex<Option<std::net::TcpListener>>>),
    /// TCP connection (Option allows close to invalidate)
    TcpConnection(Arc<Mutex<Option<std::net::TcpStream>>>),
    /// SIMD f32x8 (8-wide f32 vector for SIMD operations)
    SimdF32x8([f32; 8]),
    /// Random number generator (xoshiro256++ state)
    Rng(Arc<Mutex<RngState>>),
    /// Lazy iterator (wraps a source and optional adapters)
    Iterator(Arc<Mutex<IteratorState>>),
}

/// xoshiro256++ PRNG state.
#[derive(Debug, Clone)]
pub struct RngState {
    s: [u64; 4],
}

impl RngState {
    pub fn from_seed(seed: u64) -> Self {
        // SplitMix64 to expand seed into 4 state words
        let mut z = seed;
        let mut s = [0u64; 4];
        for slot in &mut s {
            z = z.wrapping_add(0x9e3779b97f4a7c15);
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            *slot = z ^ (z >> 31);
        }
        Self { s }
    }

    pub fn from_system() -> Self {
        use std::time::SystemTime;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42);
        Self::from_seed(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = (self.s[0].wrapping_add(self.s[3]))
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

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32)
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        if lo >= hi { return lo; }
        let range = (hi - lo) as u64;
        lo + (self.next_u64() % range) as i64
    }
}

/// Lazy iterator state. Each variant wraps a source and advances on `next()`.
pub enum IteratorState {
    /// Iterate over Vec elements by index.
    Vec {
        items: Arc<Mutex<std::vec::Vec<Value>>>,
        index: usize,
    },
    /// Apply a mapping function to each element.
    Map {
        source: Arc<Mutex<IteratorState>>,
        mapper: Value,
    },
    /// Keep only elements matching a predicate.
    Filter {
        source: Arc<Mutex<IteratorState>>,
        predicate: Value,
    },
    /// Yield (index, element) pairs.
    Enumerate {
        source: Arc<Mutex<IteratorState>>,
        counter: usize,
    },
    /// Take at most N elements.
    Take {
        source: Arc<Mutex<IteratorState>>,
        remaining: usize,
    },
    /// Skip the first N elements.
    Skip {
        source: Arc<Mutex<IteratorState>>,
        to_skip: usize,
        skipped: bool,
    },
    /// Iterate over a range of integers.
    Range {
        current: i64,
        end: i64,
        inclusive: bool,
    },
    /// Map then flatten each result.
    FlatMap {
        source: Arc<Mutex<IteratorState>>,
        mapper: Value,
        buffer: std::vec::Vec<Value>,
    },
    /// Zip two iterators together.
    Zip {
        a: Arc<Mutex<IteratorState>>,
        b: Arc<Mutex<IteratorState>>,
    },
}

impl fmt::Debug for IteratorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vec { index, .. } => write!(f, "VecIter(index={})", index),
            Self::Map { .. } => write!(f, "MapIter"),
            Self::Filter { .. } => write!(f, "FilterIter"),
            Self::Enumerate { counter, .. } => write!(f, "EnumerateIter({})", counter),
            Self::Take { remaining, .. } => write!(f, "TakeIter({})", remaining),
            Self::Skip { to_skip, .. } => write!(f, "SkipIter({})", to_skip),
            Self::Range { current, end, .. } => write!(f, "RangeIter({}..{})", current, end),
            Self::FlatMap { .. } => write!(f, "FlatMapIter"),
            Self::Zip { .. } => write!(f, "ZipIter"),
        }
    }
}

impl Value {
    /// Get the type name for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Bool(_) => "bool",
            Value::Int(_) => "i64",
            Value::Int128(_) => "i128",
            Value::Uint128(_) => "u128",
            Value::Float(_) => "f64",
            Value::Char(_) => "char",
            Value::String(_) => "string",
            Value::Struct { .. } => "struct",
            Value::Enum { .. } => "enum",
            Value::Function { .. } => "func",
            Value::Builtin(_) => "builtin",
            Value::Range { .. } => "range",
            Value::Vec(_) => "Vec",
            Value::TypeConstructor { .. } => "type",
            Value::EnumConstructor { .. } => "enum constructor",
            Value::Module(_) => "module",
            Value::Package(_) => "package",
            Value::File(_) => "File",
            Value::Closure { .. } => "closure",
            Value::Duration(_) => "Duration",
            Value::Instant(_) => "Instant",
            Value::Type(_) => "type",
            Value::Pool(_) => "Pool",
            Value::Handle { .. } => "Handle",
            Value::ThreadHandle(_) => "ThreadHandle",
            Value::TaskHandle(_) => "TaskHandle",
            Value::MultitaskingRuntime(_) => "MultitaskingRuntime",
            Value::Sender(_) => "Sender",
            Value::Receiver(_) => "Receiver",
            Value::ThreadPool(_) => "ThreadPool",
            Value::Map(_) => "Map",
            Value::AtomicBool(_) => "Atomic<bool>",
            Value::AtomicUsize(_) => "Atomic<usize>",
            Value::AtomicU64(_) => "Atomic<u64>",
            Value::Shared(_) => "Shared",
            Value::TcpListener(_) => "TcpListener",
            Value::TcpConnection(_) => "TcpConnection",
            Value::SimdF32x8(_) => "f32x8",
            Value::Rng(_) => "Rng",
            Value::Iterator(_) => "Iterator",
        }
    }

    /// Deep clone a value — creates independent copies of reference-counted internals.
    pub fn deep_clone(&self) -> Value {
        match self {
            Value::String(s) => Value::String(Arc::new(Mutex::new(s.lock().unwrap().clone()))),
            Value::Vec(v) => {
                let deep: Vec<Value> = v.lock().unwrap().iter().map(|val| val.deep_clone()).collect();
                Value::Vec(Arc::new(Mutex::new(deep)))
            }
            Value::Struct { name, fields, resource_id } => {
                let deep_fields: HashMap<String, Value> = fields.iter()
                    .map(|(k, v)| (k.clone(), v.deep_clone()))
                    .collect();
                Value::Struct { name: name.clone(), fields: deep_fields, resource_id: *resource_id }
            }
            Value::Enum { name, variant, fields } => {
                Value::Enum {
                    name: name.clone(),
                    variant: variant.clone(),
                    fields: fields.iter().map(|f| f.deep_clone()).collect(),
                }
            }
            Value::Pool(p) => {
                let pool = p.lock().unwrap();
                let mut new_pool = PoolData::new();
                new_pool.slots = pool.slots.iter().map(|(gen, opt)| {
                    (*gen, opt.as_ref().map(|v| v.deep_clone()))
                }).collect();
                new_pool.free_list = pool.free_list.clone();
                new_pool.len = pool.len;
                new_pool.type_param = pool.type_param.clone();
                Value::Pool(Arc::new(Mutex::new(new_pool)))
            }
            Value::Closure { params, body, captured_env } => {
                let deep_env: HashMap<String, Value> = captured_env.iter()
                    .map(|(k, v)| (k.clone(), v.deep_clone()))
                    .collect();
                Value::Closure { params: params.clone(), body: body.clone(), captured_env: deep_env }
            }
            Value::Map(m) => {
                let map = m.lock().unwrap();
                let deep: Vec<(Value, Value)> = map.iter()
                    .map(|(k, v)| (k.deep_clone(), v.deep_clone()))
                    .collect();
                Value::Map(Arc::new(Mutex::new(deep)))
            }
            // Value types — regular clone is sufficient
            other => other.clone(),
        }
    }

    /// Extract u64 from Value::Int (for Duration constructors).
    pub fn as_int(&self) -> Result<i64, String> {
        match self {
            Value::Int(n) => Ok(*n),
            _ => Err(format!("Expected integer, found {}", self.type_name())),
        }
    }

    pub fn as_u64(&self) -> Result<u64, String> {
        match self {
            Value::Int(n) if *n >= 0 => Ok(*n as u64),
            Value::Int(n) => Err(format!("Cannot convert negative integer {} to u64", n)),
            _ => Err(format!("Expected integer, found {}", self.type_name())),
        }
    }

    /// Extract f64 from Value::Float (for Duration.from_secs_f64).
    pub fn as_f64(&self) -> Result<f64, String> {
        match self {
            Value::Float(f) => Ok(*f),
            Value::Int(n) => Ok(*n as f64),
            _ => Err(format!("Expected float, found {}", self.type_name())),
        }
    }

    /// Get the resource ID if this value is a tracked resource.
    pub fn resource_id(&self) -> Option<u64> {
        match self {
            Value::Struct { resource_id, .. } => *resource_id,
            _ => None,
        }
    }

    /// Extract Duration nanos from Value::Duration.
    pub fn as_duration(&self) -> Result<u64, String> {
        match self {
            Value::Duration(nanos) => Ok(*nanos),
            _ => Err(format!("Expected Duration, found {}", self.type_name())),
        }
    }

    /// Extract Instant from Value::Instant.
    pub fn as_instant(&self) -> Result<std::time::Instant, String> {
        match self {
            Value::Instant(instant) => Ok(*instant),
            _ => Err(format!("Expected Instant, found {}", self.type_name())),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => write!(f, "()"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(n) => write!(f, "{}", n),
            Value::Int128(n) => write!(f, "{}", n),
            Value::Uint128(n) => write!(f, "{}", n),
            Value::Float(n) => write!(f, "{}", n),
            Value::Char(c) => write!(f, "{}", c),
            Value::String(s) => write!(f, "{}", s.lock().unwrap()),
            Value::Struct { name, fields, .. } => {
                write!(f, "{} {{ ", name)?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, " }}")
            }
            Value::Enum { name, variant, fields } => {
                write!(f, "{}.{}", name, variant)?;
                if !fields.is_empty() {
                    write!(f, "(")?;
                    for (i, v) in fields.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", v)?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
            Value::Function { name } => write!(f, "<func {}>", name),
            Value::Builtin(kind) => write!(f, "<builtin {:?}>", kind),
            Value::Range { start, end, inclusive } => {
                if *inclusive {
                    write!(f, "{}..={}", start, end)
                } else {
                    write!(f, "{}..{}", start, end)
                }
            }
            Value::Vec(v) => {
                let vec = v.lock().unwrap();
                write!(f, "[")?;
                for (i, item) in vec.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
            Value::TypeConstructor { kind, type_param } => {
                let base_name = match kind {
                    TypeConstructorKind::Vec => "Vec",
                    TypeConstructorKind::Map => "Map",
                    TypeConstructorKind::String => "string",
                    TypeConstructorKind::Pool => "Pool",
                    TypeConstructorKind::Channel => "Channel",
                    TypeConstructorKind::Shared => "Shared",
                    TypeConstructorKind::Atomic => "Atomic",
                    TypeConstructorKind::Ordering => "Ordering",
                };
                if let Some(param) = type_param {
                    write!(f, "{}<{}>", base_name, param)
                } else {
                    write!(f, "{}", base_name)
                }
            },
            Value::EnumConstructor {
                enum_name,
                variant_name,
                ..
            } => {
                write!(f, "{}.{}", enum_name, variant_name)
            }
            Value::Module(kind) => match kind {
                ModuleKind::Fs => write!(f, "<module fs>"),
                ModuleKind::Io => write!(f, "<module io>"),
                ModuleKind::Cli => write!(f, "<module cli>"),
                ModuleKind::Std => write!(f, "<module std>"),
                ModuleKind::Env => write!(f, "<module env>"),
                ModuleKind::Time => write!(f, "<module time>"),
                ModuleKind::Random => write!(f, "<module random>"),
                ModuleKind::Math => write!(f, "<module math>"),
                ModuleKind::Os => write!(f, "<module os>"),
                ModuleKind::Json => write!(f, "<module json>"),
                ModuleKind::Path => write!(f, "<module path>"),
                ModuleKind::Net => write!(f, "<module net>"),
                ModuleKind::Async => write!(f, "<module async>"),
                ModuleKind::Thread => write!(f, "<module thread>"),
            },
            Value::Package(name) => write!(f, "<package {}>", name),
            Value::File(file) => {
                if file.lock().unwrap().is_some() {
                    write!(f, "<file>")
                } else {
                    write!(f, "<closed file>")
                }
            }
            Value::Closure { params, .. } => {
                write!(f, "<closure |{}|>", params.join(", "))
            }
            Value::Duration(nanos) => {
                if *nanos >= 1_000_000_000 {
                    write!(f, "{}s", *nanos / 1_000_000_000)
                } else if *nanos >= 1_000_000 {
                    write!(f, "{}ms", *nanos / 1_000_000)
                } else if *nanos >= 1_000 {
                    write!(f, "{}μs", *nanos / 1_000)
                } else {
                    write!(f, "{}ns", *nanos)
                }
            }
            Value::Instant(_) => write!(f, "<Instant>"),
            Value::Type(name) => write!(f, "<type {}>", name),
            Value::Pool(p) => {
                let pool = p.lock().unwrap();
                write!(f, "<Pool len={}>", pool.len)
            }
            Value::Handle {
                pool_id,
                index,
                generation,
            } => write!(f, "Handle({}, {}, {})", pool_id, index, generation),
            Value::ThreadHandle(_) => write!(f, "<ThreadHandle>"),
            Value::TaskHandle(_) => write!(f, "<TaskHandle>"),
            Value::MultitaskingRuntime(r) => write!(f, "<Multitasking runtime workers={}>", r.workers),
            Value::Sender(_) => write!(f, "<Sender>"),
            Value::Receiver(_) => write!(f, "<Receiver>"),
            Value::ThreadPool(p) => write!(f, "<ThreadPool size={}>", p.size),
            Value::Map(m) => {
                let map = m.lock().unwrap();
                write!(f, "Map {{ ")?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, " }}")
            }
            Value::Shared(s) => {
                let inner = s.read().unwrap();
                write!(f, "Shared({})", inner)
            }
            Value::AtomicBool(a) => {
                write!(f, "Atomic<bool>({})", a.load(std::sync::atomic::Ordering::Relaxed))
            }
            Value::AtomicUsize(a) => {
                write!(f, "Atomic<usize>({})", a.load(std::sync::atomic::Ordering::Relaxed))
            }
            Value::AtomicU64(a) => {
                write!(f, "Atomic<u64>({})", a.load(std::sync::atomic::Ordering::Relaxed))
            }
            Value::TcpListener(l) => {
                if l.lock().unwrap().is_some() {
                    write!(f, "<TcpListener>")
                } else {
                    write!(f, "<closed TcpListener>")
                }
            }
            Value::TcpConnection(c) => {
                if c.lock().unwrap().is_some() {
                    write!(f, "<TcpConnection>")
                } else {
                    write!(f, "<closed TcpConnection>")
                }
            }
            Value::SimdF32x8(v) => {
                write!(f, "f32x8(")?;
                for (i, x) in v.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", x)?;
                }
                write!(f, ")")
            }
            Value::Rng(_) => write!(f, "<Rng>"),
            Value::Iterator(_) => write!(f, "<Iterator>"),
        }
    }
}
