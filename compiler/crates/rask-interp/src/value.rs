//! Runtime values.

use std::collections::HashMap;
use std::fmt;
use std::fs::File as StdFile;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};

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
}

impl PoolData {
    pub fn new() -> Self {
        Self {
            pool_id: next_pool_id(),
            slots: Vec::new(),
            free_list: Vec::new(),
            len: 0,
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
}

/// Type constructor kinds (for static method calls like Vec.new()).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeConstructorKind {
    Vec,
    Map,
    String,
    Pool,
    Channel,
}

/// Module kinds for stdlib modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Fs,   // fs.read_file, fs.write_file, etc.
    Io,   // io.read_line, io.print, etc.
    Cli,  // cli.args
    Std,  // std.exit
    Env,  // env.var, env.vars
    Time,   // time.Instant, time.Duration, time.sleep
    Random, // random.f32, random.f64, random.range, etc.
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

/// A runtime value in the interpreter.
#[derive(Debug, Clone)]
pub enum Value {
    /// Unit value
    Unit,
    /// Boolean
    Bool(bool),
    /// Integer (using i64 for all integer types in interpreter)
    Int(i64),
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
    TypeConstructor(TypeConstructorKind),
    /// Enum variant constructor (e.g., Option.Some before calling with args)
    EnumConstructor {
        enum_name: String,
        variant_name: String,
        field_count: usize,
    },
    /// Module (fs, io, cli, std, env)
    Module(ModuleKind),
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
    /// Thread pool (from `with threading(n)`)
    ThreadPool(Arc<ThreadPoolInner>),
}

impl Value {
    /// Get the type name for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Bool(_) => "bool",
            Value::Int(_) => "i64",
            Value::Float(_) => "f64",
            Value::Char(_) => "char",
            Value::String(_) => "string",
            Value::Struct { .. } => "struct",
            Value::Enum { .. } => "enum",
            Value::Function { .. } => "func",
            Value::Builtin(_) => "builtin",
            Value::Range { .. } => "range",
            Value::Vec(_) => "Vec",
            Value::TypeConstructor(_) => "type",
            Value::EnumConstructor { .. } => "enum constructor",
            Value::Module(_) => "module",
            Value::File(_) => "File",
            Value::Closure { .. } => "closure",
            Value::Duration(_) => "Duration",
            Value::Instant(_) => "Instant",
            Value::Type(_) => "type",
            Value::Pool(_) => "Pool",
            Value::Handle { .. } => "Handle",
            Value::ThreadHandle(_) => "ThreadHandle",
            Value::Sender(_) => "Sender",
            Value::Receiver(_) => "Receiver",
            Value::ThreadPool(_) => "ThreadPool",
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
            Value::TypeConstructor(kind) => match kind {
                TypeConstructorKind::Vec => write!(f, "Vec"),
                TypeConstructorKind::Map => write!(f, "Map"),
                TypeConstructorKind::String => write!(f, "string"),
                TypeConstructorKind::Pool => write!(f, "Pool"),
                TypeConstructorKind::Channel => write!(f, "Channel"),
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
            },
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
            Value::Sender(_) => write!(f, "<Sender>"),
            Value::Receiver(_) => write!(f, "<Receiver>"),
            Value::ThreadPool(p) => write!(f, "<ThreadPool size={}>", p.size),
        }
    }
}
