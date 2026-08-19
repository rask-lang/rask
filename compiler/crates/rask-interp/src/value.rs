// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Runtime values.

use std::collections::HashMap;
use indexmap::IndexMap;
use std::fmt;
use std::fs::File as StdFile;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};
use std::sync::LazyLock;

use rask_ast::expr::Expr;

/// Width and signedness carried by `Value::Int`, so integer arithmetic is
/// self-describing (type.overflow). `Untyped` means the width wasn't known at
/// the value's creation (e.g. an internally-produced length or index) and is
/// treated as i64 with no overflow check. Concrete kinds are range-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntKind {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Untyped,
}

impl IntKind {
    /// P2: what `usize` is on this target — pointer-sized. The width comes
    /// from `rask_ast::primitives::pointer_bits`, the one place that decides it.
    pub fn usize_kind() -> IntKind {
        if rask_ast::primitives::pointer_bits() == 32 { IntKind::U32 } else { IntKind::U64 }
    }

    /// P2: what `isize` is on this target.
    pub fn isize_kind() -> IntKind {
        if rask_ast::primitives::pointer_bits() == 32 { IntKind::I32 } else { IntKind::I64 }
    }

    /// Unsigned widths carry their value as a bit pattern in the signed slot.
    pub fn is_unsigned(self) -> bool {
        matches!(self, IntKind::U8 | IntKind::U16 | IntKind::U32 | IntKind::U64)
    }

    /// Map a checker type to an int kind. Non-integers and i128/u128 (their own
    /// `Value` variants) map to `Untyped`.
    pub fn from_type(ty: &rask_types::Type) -> IntKind {
        use rask_types::Type;
        match ty {
            Type::I8 => IntKind::I8,
            Type::I16 => IntKind::I16,
            Type::I32 => IntKind::I32,
            Type::I64 => IntKind::I64,
            Type::U8 => IntKind::U8,
            Type::U16 => IntKind::U16,
            Type::U32 => IntKind::U32,
            Type::U64 => IntKind::U64,
            _ => IntKind::Untyped,
        }
    }

    pub fn signed(self) -> bool {
        matches!(self, IntKind::I8 | IntKind::I16 | IntKind::I32 | IntKind::I64 | IntKind::Untyped)
    }

    /// Bit width, or None for `Untyped` (no fixed width → unchecked).
    pub fn bits(self) -> Option<u32> {
        Some(match self {
            IntKind::I8 | IntKind::U8 => 8,
            IntKind::I16 | IntKind::U16 => 16,
            IntKind::I32 | IntKind::U32 => 32,
            IntKind::I64 | IntKind::U64 => 64,
            IntKind::Untyped => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            IntKind::I8 => "i8",
            IntKind::I16 => "i16",
            IntKind::I32 => "i32",
            IntKind::I64 => "i64",
            IntKind::U8 => "u8",
            IntKind::U16 => "u16",
            IntKind::U32 => "u32",
            IntKind::U64 => "u64",
            IntKind::Untyped => "int",
        }
    }

    /// Map a type-name string (as used by `as` casts) to an int kind.
    pub fn from_name(s: &str) -> Option<IntKind> {
        Some(match s {
            "i8" => IntKind::I8,
            "i16" => IntKind::I16,
            "i32" | "int" => IntKind::I32,
            "i64" => IntKind::I64,
            "isize" => IntKind::isize_kind(),
            "u8" => IntKind::U8,
            "u16" => IntKind::U16,
            "u32" => IntKind::U32,
            "u64" | "uint" => IntKind::U64,
            "usize" => IntKind::usize_kind(),
            _ => return None,
        })
    }

    /// Mask/sign-extend an i64 into this kind's width (the value a cast to this
    /// kind must hold). Untyped and 64-bit kinds pass through.
    pub fn wrap(self, n: i64) -> i64 {
        let bits = match self.bits() {
            Some(b) if b < 64 => b,
            _ => return n,
        };
        let mask = (1i128 << bits) - 1;
        let masked = (n as i128) & mask;
        let v = if self.signed() && (masked & (1i128 << (bits - 1))) != 0 {
            masked - (1i128 << bits)
        } else {
            masked
        };
        v as i64
    }

    /// Pick the more specific of two kinds — arithmetic operands share a type,
    /// but one side may be untyped (e.g. a generic constant).
    pub fn unify(self, other: IntKind) -> IntKind {
        match (self, other) {
            (IntKind::Untyped, k) | (k, IntKind::Untyped) => k,
            (a, _) => a,
        }
    }
}

/// Width carried by `Value::Float`, the float counterpart of `IntKind`.
///
/// Every float is stored in an `f64` slot, so without this tag an `f32`
/// computation kept ~29 bits of mantissa it doesn't have and drifted away from
/// native, where the same program does true 32-bit arithmetic. `round` puts the
/// result back on the f32 grid after each operation.
///
/// A single `+ - * /` or `sqrt` done in f64 and then rounded to f32 gives
/// exactly the f32 answer — f64 carries more than twice f32's mantissa, so
/// there's no double-rounding error to worry about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatKind {
    F32,
    F64,
    /// Width not known where the value was made (an internal constant, a
    /// stdlib return). Treated as f64.
    Untyped,
}

impl FloatKind {
    pub fn from_type(ty: &rask_types::Type) -> FloatKind {
        use rask_types::Type;
        match ty {
            Type::F32 => FloatKind::F32,
            Type::F64 => FloatKind::F64,
            _ => FloatKind::Untyped,
        }
    }

    pub fn from_name(s: &str) -> Option<FloatKind> {
        Some(match s {
            "f32" => FloatKind::F32,
            "f64" => FloatKind::F64,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            FloatKind::F32 => "f32",
            FloatKind::F64 | FloatKind::Untyped => "f64",
        }
    }

    /// Snap a value onto this width's grid. The f64 cases are identity.
    pub fn round(self, v: f64) -> f64 {
        match self {
            FloatKind::F32 => v as f32 as f64,
            FloatKind::F64 | FloatKind::Untyped => v,
        }
    }

    /// An untyped operand takes the other side's width, matching `IntKind`.
    pub fn unify(self, other: FloatKind) -> FloatKind {
        match (self, other) {
            (FloatKind::Untyped, k) | (k, FloatKind::Untyped) => k,
            (a, _) => a,
        }
    }

    /// Spell a value at this width. f64 uses Rust's own shortest form; f32
    /// mirrors `rask_fmt_float` in the C runtime so both backends print a
    /// float the same way.
    pub fn format(self, v: f64) -> String {
        match self {
            FloatKind::F32 => format_f32(v as f32),
            FloatKind::F64 | FloatKind::Untyped => v.to_string(),
        }
    }
}

/// The shortest decimal that reads back as the same `f32`, spelled out with no
/// exponent — the same walk the C runtime does.
///
/// Rust's own `{}` on an f32 also round-trips, but picks a different spelling
/// for some values (123456792f32 comes out as `123456790`), so matching native
/// means matching its algorithm rather than substituting an equivalent one.
fn format_f32(val: f32) -> String {
    if val.is_nan() {
        return "NaN".to_string();
    }
    if val.is_infinite() {
        return if val < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if val == 0.0 {
        return if val.is_sign_negative() { "-0" } else { "0" }.to_string();
    }

    let d = val as f64;
    // Fewest significant digits that still read back as this f32.
    let mut prec = 1;
    while prec < 9 {
        if format!("{:.*e}", prec - 1, d).parse::<f32>() == Ok(val) {
            break;
        }
        prec += 1;
    }

    // Take the decimal exponent from the same rendering rather than log10,
    // which is off by one at exact powers of ten.
    let rendered = format!("{:.*e}", prec - 1, d);
    let exp10: i32 = rendered
        .split('e')
        .nth(1)
        .and_then(|e| e.parse().ok())
        .unwrap_or(0);

    let decimals = (prec as i32 - 1 - exp10).clamp(0, 60) as usize;
    let out = format!("{:.*}", decimals, d);
    if out.contains('.') {
        out.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        out
    }
}

/// Global pool ID counter. Each Pool gets a unique ID.
static NEXT_POOL_ID: AtomicU32 = AtomicU32::new(1);

/// Process-global active Multitasking runtime slot (conc.async/C1).
/// At most one `using Multitasking { }` block may be active per process.
pub static ACTIVE_RUNTIME: LazyLock<RwLock<Option<Arc<MultitaskingRuntime>>>> =
    LazyLock::new(|| RwLock::new(None));

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
    /// mem.pools/PL2: capacity bound. `None` = unbounded (grows on demand);
    /// `Some(n)` = a `with_capacity(n)` pool that never exceeds `n` live elements.
    pub capacity: Option<usize>,
}

impl PoolData {
    pub fn new() -> Self {
        Self {
            pool_id: next_pool_id(),
            slots: Vec::new(),
            free_list: Vec::new(),
            len: 0,
            type_param: None,
            capacity: None,
        }
    }

    pub fn with_type_param(type_param: Option<String>) -> Self {
        Self {
            pool_id: next_pool_id(),
            slots: Vec::new(),
            free_list: Vec::new(),
            len: 0,
            type_param,
            capacity: None,
        }
    }

    /// mem.pools/PL8: a bounded pool at its capacity limit rejects new inserts.
    pub fn is_full(&self) -> bool {
        self.capacity.map_or(false, |cap| self.len >= cap)
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

/// Global store ID counter. Each Store gets a unique ID.
static NEXT_STORE_ID: AtomicU32 = AtomicU32::new(1);

pub fn next_store_id() -> u32 {
    NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Live stores by id, weakly held.
///
/// Root-edge registration needs to get from a link to its store at the moment
/// the link is written into an outside field. In the real design that's static
/// — the compiler knows which field targets which store — so this registry is
/// prototype scaffolding, not part of the model. Nothing on the read path
/// touches it.
static STORE_REGISTRY: LazyLock<Mutex<HashMap<u32, Weak<Mutex<StoreData>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register_store(store: &Arc<Mutex<StoreData>>) {
    let id = store.lock().unwrap().store_id;
    let mut reg = STORE_REGISTRY.lock().unwrap();
    reg.retain(|_, w| w.strong_count() > 0);
    reg.insert(id, Arc::downgrade(store));
}

pub fn store_by_id(id: u32) -> Option<Arc<Mutex<StoreData>>> {
    STORE_REGISTRY.lock().unwrap().get(&id).and_then(|w| w.upgrade())
}

/// Which slot holds an edge, for dedup and for unlinking on overwrite.
///
/// Owned rather than borrowed so it can key a map: registration is then O(1),
/// which matters because a hub with N incoming edges gets N registrations and a
/// linear dedup scan would make building it quadratic.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BacklinkKey {
    /// Holder allocation address. Stable for the allocation's life.
    pub holder: usize,
    /// Field name for a struct slot; `None` for a container, whose backlink
    /// names the container rather than a position.
    pub field: Option<String>,
}

/// One place that holds an edge — a *backlink*. The store keeps, per node, the
/// set of places pointing at it, so `delete` can find and fix every incoming
/// edge without looking at anything else (analysis.fourth-option, rule 2).
///
/// Three kinds because an edge can be held three ways: a scalar field
/// (`target: Link<T>?`), an element of an edge list (`children: Vec<Link<T>>`),
/// or a value in an index (`by_name: Map<K, Link<T>>`). A node field and a root
/// field are the same kind here — a root edge is just an edge whose holder
/// happens to live outside the store, which is why root edges need no separate
/// mechanism.
///
/// Weak throughout: recording a backlink must not keep the holder alive.
///
/// A struct slot is named exactly, so overwriting `a.target` unlinks the old
/// target's backlink precisely. A container backlink names the container and no
/// position, because positions shift under insertion and rehashing — so it is
/// one entry per (container, target) pair however many elements match, and it is
/// dropped when a fixup visit finds the container no longer holds that edge.
#[derive(Debug, Clone)]
pub enum Backlink {
    /// A named field of a struct — a node's edge, or a root edge.
    Field(Weak<Mutex<StructData>>, String),
    /// An element of an edge list.
    Element(Weak<Mutex<VecData>>),
    /// A value in an index.
    Entry(Weak<Mutex<MapData>>),
}

impl Backlink {
    pub fn key(&self) -> BacklinkKey {
        match self {
            Backlink::Field(w, f) => BacklinkKey {
                holder: w.as_ptr() as usize,
                field: Some(f.clone()),
            },
            Backlink::Element(w) => BacklinkKey { holder: w.as_ptr() as usize, field: None },
            Backlink::Entry(w) => BacklinkKey { holder: w.as_ptr() as usize, field: None },
        }
    }
}

/// Node identity used to key the backlink index. A link carries the node's
/// `Arc`, so this is available at O(1) wherever a link is.
///
/// Sound as a key because the store holds a strong reference to every live
/// node: the allocation can't be freed and its address reused while the node is
/// still in the store, so two live nodes never share a key.
pub fn node_key(node: &Arc<Mutex<StructData>>) -> usize {
    Arc::as_ptr(node) as usize
}

/// Internal store storage — the arena half of `Store<T>` + `Link<T>`.
///
/// Unlike `PoolData` there are no generation counters, because there is no
/// stale state to detect: `delete` walks every incoming edge and nulls it, so
/// a link is either absent or valid. Slots hold the node's `Arc<Mutex<StructData>>`,
/// and a `Value::Link` holds that same Arc — following a link is a pointer
/// deref with nothing to check.
#[derive(Debug, Clone)]
pub struct StoreData {
    pub store_id: u32,
    /// Live nodes, in insertion order. `None` marks a freed slot.
    pub slots: Vec<Option<Arc<Mutex<StructData>>>>,
    pub free_list: Vec<u32>,
    pub len: usize,
    pub type_param: Option<String>,
    /// Incoming edges per node: who points at me. This is what makes `delete`
    /// cost O(in-degree) instead of a scan.
    /// Keyed by slot so registration and unlinking are both O(1) — a hub with
    /// N incoming edges takes N registrations, and a linear dedup scan would
    /// make building it quadratic. `IndexMap` rather than `HashMap` so the
    /// fixup visits holders in a deterministic order.
    pub incoming: HashMap<usize, IndexMap<BacklinkKey, Backlink>>,
    /// Slot index per node, so `delete` doesn't scan `slots` to find one.
    pub slot_of: HashMap<usize, u32>,
    /// For a snapshot: the store it was copied from, and the node it copied each
    /// of its own nodes from. `corresponding` uses these to translate a link the
    /// caller still holds into the equivalent node over here.
    pub origin_id: Option<u32>,
    pub origin: HashMap<usize, Arc<Mutex<StructData>>>,
}

impl StoreData {
    pub fn new() -> Self {
        Self {
            store_id: next_store_id(),
            slots: Vec::new(),
            free_list: Vec::new(),
            len: 0,
            type_param: None,
            incoming: HashMap::new(),
            slot_of: HashMap::new(),
            origin_id: None,
            origin: HashMap::new(),
        }
    }

    pub fn with_type_param(type_param: Option<String>) -> Self {
        Self { type_param, ..Self::new() }
    }

    /// Insert a node, returning its slot index.
    pub fn insert(&mut self, node: Arc<Mutex<StructData>>) -> u32 {
        self.len += 1;
        let key = node_key(&node);
        let idx = if let Some(free_idx) = self.free_list.pop() {
            self.slots[free_idx as usize] = Some(node);
            free_idx
        } else {
            self.slots.push(Some(node));
            (self.slots.len() - 1) as u32
        };
        self.slot_of.insert(key, idx);
        // A reused slot must not inherit the previous occupant's backlinks.
        self.incoming.remove(&key);
        idx
    }

    /// Slot index of a node, or `None` if it isn't in this store. O(1).
    pub fn index_of(&self, node: &Arc<Mutex<StructData>>) -> Option<usize> {
        self.slot_of.get(&node_key(node)).map(|i| *i as usize)
    }

    /// Record that `holder` points at `target`. Idempotent, and drops entries
    /// whose holder has died.
    ///
    /// Over-approximating is safe: a backlink left behind after its edge was
    /// overwritten costs the fixup one wasted visit, because the fixup re-checks
    /// that the slot really points at the dying node before rewriting it. A
    /// *missing* backlink would be unsound, so registration errs toward
    /// recording.
    pub fn register_backlink(&mut self, target: &Arc<Mutex<StructData>>, holder: Backlink) {
        self.incoming
            .entry(node_key(target))
            .or_default()
            .insert(holder.key(), holder);
    }

    /// Forget that `slot` points at `target` — the old half of an overwrite.
    ///
    /// Exact for a struct field, which names its slot. A container slot names
    /// only the container, so its backlink is dropped when the container stops
    /// holding *any* edge to the target, not when one element changes.
    pub fn unregister_backlink(&mut self, target: &Arc<Mutex<StructData>>, slot: &BacklinkKey) {
        let key = node_key(target);
        if let Some(entry) = self.incoming.get_mut(&key) {
            entry.shift_remove(slot);
            if entry.is_empty() {
                self.incoming.remove(&key);
            }
        }
    }

    /// Take a node's incoming-edge list, for the delete that is about to fix it.
    pub fn take_incoming(&mut self, node: &Arc<Mutex<StructData>>) -> Vec<Backlink> {
        self.incoming
            .remove(&node_key(node))
            .map(|m| m.into_values().collect())
            .unwrap_or_default()
    }

    pub fn live_nodes(&self) -> Vec<Arc<Mutex<StructData>>> {
        self.slots.iter().flatten().cloned().collect()
    }
}

/// Built-in function kinds (global functions without module prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Print,
    Println,
    EPrint,   // eprint(...) — same as print, to stderr
    EPrintln, // eprintln(...) — same as println, to stderr
    Panic,
    Format,
    AsyncSpawn,     // spawn(|| {}) from async module
    JoinAll,        // join_all(handles) — wait for all tasks
    SelectFirst,    // select_first(handles) — first completed wins
    Cancelled,      // cancelled() — cooperative cancellation check
    Todo,
    Unreachable,
    Min,   // generic min(a, b) — prelude
    Max,   // generic max(a, b) — prelude
    Clamp, // generic clamp(value, lo, hi) — prelude
    AssertEq,   // assert_eq(got, expected) — pretty-print diff on failure
    Skip,       // skip("reason") — skip rest of test
    ExpectFail, // expect_fail() — invert pass/fail
}

/// Type constructor kinds (for static method calls like Vec.new()).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeConstructorKind {
    Vec,
    Map,
    String,
    Char,
    Pool,
    Store,
    Cell,
    Channel,
    Shared,
    Mutex,
    Atomic,
    Ordering,
    TaskGroup,
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
    Http,    // http.serve, http.get, etc.
    Reflect, // std.reflect — compile-time type introspection
}

impl ModuleKind {
    /// The name this module is imported under, and the stem of its stdlib file.
    pub fn name(self) -> &'static str {
        match self {
            ModuleKind::Fs => "fs",
            ModuleKind::Io => "io",
            ModuleKind::Cli => "cli",
            ModuleKind::Std => "std",
            ModuleKind::Env => "env",
            ModuleKind::Time => "time",
            ModuleKind::Random => "random",
            ModuleKind::Math => "math",
            ModuleKind::Os => "os",
            ModuleKind::Json => "json",
            ModuleKind::Path => "path",
            ModuleKind::Net => "net",
            ModuleKind::Async => "async",
            ModuleKind::Thread => "thread",
            ModuleKind::Http => "http",
            ModuleKind::Reflect => "reflect",
        }
    }

    /// The module a name imports, if it's a stdlib module.
    pub fn from_name(name: &str) -> Option<ModuleKind> {
        ALL_MODULE_KINDS.iter().copied().find(|m| m.name() == name)
    }

    /// Types and enums this module brings into scope.
    ///
    /// Comes from `rask_stdlib::modules`, which reads the module's own `.rk`
    /// file — the same table the resolver uses. The interpreter used to keep
    /// three lists of its own for the three import spellings, and they
    /// disagreed: `http`'s types were reachable only through a glob import.
    pub fn exported_types(self) -> impl Iterator<Item = &'static str> {
        let e = rask_stdlib::modules::exports(self.name());
        e.types
            .iter()
            .map(String::as_str)
            .chain(e.enums.iter().map(|(n, _): &(String, Vec<String>)| n.as_str()))
    }

    /// True when `name` is one of this module's exported types.
    pub fn exports_type(self, name: &str) -> bool {
        rask_stdlib::modules::exports_type(self.name(), name)
    }
}

/// Every module kind. `from_name` walks this, so a new variant is reachable as
/// soon as it has a name.
pub const ALL_MODULE_KINDS: &[ModuleKind] = &[
    ModuleKind::Fs,
    ModuleKind::Io,
    ModuleKind::Cli,
    ModuleKind::Std,
    ModuleKind::Env,
    ModuleKind::Time,
    ModuleKind::Random,
    ModuleKind::Math,
    ModuleKind::Os,
    ModuleKind::Json,
    ModuleKind::Path,
    ModuleKind::Net,
    ModuleKind::Async,
    ModuleKind::Thread,
    ModuleKind::Http,
    ModuleKind::Reflect,
];

/// Inner state for a spawned thread/task handle.
pub struct ThreadHandleInner {
    /// OS thread join handle (used for raw thread::spawn)
    pub handle: Mutex<Option<std::thread::JoinHandle<Result<Value, String>>>>,
    /// Result channel (used for tasks submitted to a thread pool)
    pub receiver: Mutex<Option<mpsc::Receiver<Result<Value, String>>>>,
    /// ctrl.panic/F1: which task this is, for the detached-panic report.
    pub task_id: i64,
}

/// Task ids, handed out in spawn order like the runtime's `rask_next_task_id`.
pub fn next_task_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
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

/// Multitasking runtime — bounded thread pool for spawn() tasks.
pub struct MultitaskingRuntime {
    pub workers: usize,
    pub sender: Mutex<Option<mpsc::Sender<PoolTask>>>,
    pub pool_threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl MultitaskingRuntime {
    pub fn new(workers: usize) -> Self {
        let (tx, rx) = mpsc::channel::<PoolTask>();
        let rx = Arc::new(Mutex::new(rx));

        let mut threads = Vec::with_capacity(workers);
        for _ in 0..workers {
            let rx = Arc::clone(&rx);
            threads.push(crate::spawn_interp_thread(move || {
                loop {
                    let task = rx.lock().unwrap().recv();
                    match task {
                        Ok(task) => (task.work)(),
                        Err(_) => break, // Channel closed
                    }
                }
            }));
        }

        Self {
            workers,
            sender: Mutex::new(Some(tx)),
            pool_threads: Mutex::new(threads),
        }
    }

    /// Shut down the pool: drop sender, join all workers.
    pub fn shutdown(&self) {
        *self.sender.lock().unwrap() = None;
        let mut threads = self.pool_threads.lock().unwrap();
        for t in threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl fmt::Debug for MultitaskingRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MultitaskingRuntime(workers={})", self.workers)
    }
}

/// Struct instance data behind Arc<Mutex<>> for shared mutation.
/// IndexMap preserves field declaration order (CO3).
#[derive(Debug, Clone)]
pub struct StructData {
    pub name: String,
    pub fields: IndexMap<String, Value>,
    /// Resource tracking ID (Some for @resource types).
    pub resource_id: Option<u64>,
}

/// A Map key. Hash/Eq delegate to `Interpreter::value_hash`/`value_eq` — the
/// same structural comparison every other Value equality check in the
/// interpreter uses — so a key found by `==` is always the key a Map finds too.
#[derive(Debug, Clone)]
pub struct MapKey(pub Value);

impl PartialEq for MapKey {
    fn eq(&self, other: &Self) -> bool {
        crate::interp::Interpreter::value_eq(&self.0, &other.0)
    }
}

impl Eq for MapKey {}

impl std::hash::Hash for MapKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(crate::interp::Interpreter::value_hash(&self.0));
    }
}

/// Map's backing store. Kept insertion-ordered (`IndexMap`, not `HashMap`) so
/// internal users that need a specific order — struct-to-JSON encoding
/// walking fields in declaration order, JSON decoding preserving source order
/// — can just iterate it directly. A real Rask `Map`'s *observable* order
/// (`.keys()`, `.values()`, `.iter()`, `for`, printing, `take_all`) must not
/// be insertion order per determinism/D7 — those call sites go through
/// `map_entries_seeded` instead of iterating this directly.
///
/// json.rs is the one exception, and it's narrower than it was: a JsonValue now
/// goes to the Rask encoder in stdlib/json.rk, which iterates the Map as user
/// code does and so gets seeded order. What's left on the Rust path is a struct
/// encoded by reflection, where the fields must come out in declaration order —
/// that's what #540 was about, and it isn't a Rask `Map` being iterated.
pub type MapData = IndexMap<MapKey, Value>;

/// Per-process random value mixed into a key's hash to order a Map's
/// observable iteration (determinism/D7: seeded hash order, not insertion
/// order, not attacker-predictable). `RandomState` pulls its keys from OS
/// entropy, so this varies run to run without a hand-rolled seed source.
fn map_order_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    static SEED: LazyLock<u64> = LazyLock::new(|| {
        std::collections::hash_map::RandomState::new().build_hasher().finish()
    });
    *SEED
}

/// A Map's entries in seeded hash order — what every user-observable Map
/// operation (`.keys()`, `.values()`, `.iter()`, `for`, printing, `take_all`)
/// iterates instead of the insertion-ordered backing store. Stable for a
/// given key set within one process, but neither insertion order nor
/// guessable across processes.
pub fn map_entries_seeded(map: &MapData) -> Vec<(Value, Value)> {
    let mut entries: Vec<(Value, Value)> = map.iter()
        .map(|(k, v)| (k.0.clone(), v.clone()))
        .collect();
    let seed = map_order_seed();
    entries.sort_by_key(|(k, _)| crate::interp::Interpreter::value_hash(k) ^ seed);
    entries
}

/// A vector's elements plus its capacity bound (`std.collections/CP1-CP3`).
///
/// Derefs to the elements, so everything that just wants the contents reads and
/// writes them directly; only the bounded operations look at `bound`.
#[derive(Debug, Clone, Default)]
pub struct VecData {
    pub items: Vec<Value>,
    /// CP2: the ceiling this vector may not grow past. `None` is unbounded.
    /// `Vec.fixed(0)` is a real bound of zero, so it can't be spelled with 0.
    pub bound: Option<usize>,
}

impl VecData {
    pub fn new(items: Vec<Value>) -> Self {
        VecData { items, bound: None }
    }

    pub fn fixed(n: usize) -> Self {
        VecData { items: Vec::with_capacity(n), bound: Some(n) }
    }

    /// CP2: at the bound, and so refusing further growth.
    pub fn is_full(&self) -> bool {
        self.bound.is_some_and(|b| self.items.len() >= b)
    }

    /// Room left before the bound, or `None` when unbounded.
    pub fn remaining(&self) -> Option<usize> {
        self.bound.map(|b| b.saturating_sub(self.items.len()))
    }
}

impl From<Vec<Value>> for VecData {
    fn from(items: Vec<Value>) -> Self {
        VecData::new(items)
    }
}

impl std::ops::Deref for VecData {
    type Target = Vec<Value>;
    fn deref(&self) -> &Vec<Value> {
        &self.items
    }
}

impl std::ops::DerefMut for VecData {
    fn deref_mut(&mut self) -> &mut Vec<Value> {
        &mut self.items
    }
}

/// A runtime value in the interpreter.
#[derive(Debug, Clone)]
pub enum Value {
    /// Unit value
    Unit,
    /// Boolean
    Bool(bool),
    /// Integer stored as i64, tagged with its source width (type.overflow).
    Int(i64, IntKind),
    /// 128-bit signed integer
    Int128(i128),
    /// 128-bit unsigned integer
    Uint128(u128),
    /// Float stored as f64, tagged with its source width. The tag is what
    /// keeps `f32` arithmetic on the f32 grid instead of silently running at
    /// double precision.
    Float(f64, FloatKind),
    /// Character
    Char(char),
    /// String (mutable, like Vec)
    String(Arc<Mutex<String>>),
    /// Struct instance (shared reference for mutation through self methods)
    Struct(Arc<Mutex<StructData>>),
    /// Enum variant
    Enum {
        name: String,
        variant: String,
        fields: Vec<Value>,
        /// Variant index in declaration order (for Comparable ordering).
        variant_index: u32,
        /// Error origin: `"file.rk:42"` — set by `try` at first propagation (ER15).
        origin: Option<Arc<str>>,
    },
    /// Function reference
    Function {
        name: String,
    },
    /// Built-in function
    Builtin(BuiltinKind),
    /// Range value (for iteration). `step` and `rev` carry the ctrl.ranges
    /// adapters: `(0..10).step(2)` sets step, `.rev()` walks the same element
    /// set backwards. A plain range is step 1, not reversed.
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
        step: i64,
        rev: bool,
    },
    /// Vec (growable array) with interior mutability
    Vec(Arc<Mutex<VecData>>),
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
        variant_index: u32,
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
    /// Cell<T> (CE1–CE6: single heap-allocated mutable value)
    Cell(Arc<Mutex<Value>>),
    /// Pool (sparse storage with generation counters)
    Pool(Arc<Mutex<PoolData>>),
    /// Store (arena of nodes; edges into it are fixed at delete)
    Store(Arc<Mutex<StoreData>>),
    /// Link — one edge to a node. Holds the node pointer directly, so following
    /// it is a deref with no generation check and no store lookup. `store_id`
    /// only names the owning store for structural ops and for delete's fixup.
    Link {
        store_id: u32,
        node: Arc<Mutex<StructData>>,
    },
    /// Handle (opaque reference into a pool)
    Handle {
        pool_id: u32,
        index: u32,
        generation: u32,
    },
    /// WeakHandle (non-owning reference into a pool — may become invalid)
    WeakHandle {
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
    /// TaskGroup for dynamic task spawning (M3)
    TaskGroup(Arc<Mutex<Vec<Value>>>),
    /// Multitasking runtime (from `using Multitasking { }`)
    MultitaskingRuntime(Arc<MultitaskingRuntime>),
    /// Map (key-value storage with Value keys)
    Map(Arc<Mutex<MapData>>),
    /// Atomic bool (lock-free boolean)
    AtomicBool(Arc<std::sync::atomic::AtomicBool>),
    /// Atomic usize (lock-free unsigned integer)
    AtomicUsize(Arc<std::sync::atomic::AtomicUsize>),
    /// Atomic u64 (lock-free 64-bit unsigned integer)
    AtomicU64(Arc<std::sync::atomic::AtomicU64>),
    /// Shared<T> (RwLock wrapper for concurrent read-heavy access)
    Shared(Arc<RwLock<Value>>),
    /// Mutex<T> (exclusive lock wrapper)
    RaskMutex(Arc<Mutex<Value>>),
    /// TCP listener socket (Option allows close to invalidate)
    TcpListener(Arc<Mutex<Option<std::net::TcpListener>>>),
    /// TCP connection (Option allows close to invalidate)
    TcpConnection(Arc<Mutex<Option<std::net::TcpStream>>>),
    /// SIMD f32x8 (8-wide f32 vector for SIMD operations)
    SimdF32x8([f32; 8]),
    /// Random number generator (xoshiro256++ state)
    Rng(Arc<Mutex<RngState>>),
    /// StringBuilder's growable buffer. Shared so a `mutate self` push is
    /// visible through the caller's binding, matching the native handle.
    StringBuilder(Arc<Mutex<String>>),
    /// Lazy iterator (wraps a source and optional adapters)
    Iterator(Arc<Mutex<IteratorState>>),
    /// Wide<T> — a staged data-parallel plan (conc.data-parallel). Lazy: it
    /// records ops and runs nothing until a terminal (`read`/`sum`). The plan
    /// tree is immutable, so it's shared by Arc.
    Wide(Arc<WidePlan>),
    /// Nominal type wrapper: `type UserId = u64` — wraps the underlying value
    Nominal {
        type_name: String,
        inner: Box<Value>,
    },
    /// Nominal type constructor: makes a type name callable as `UserId(42)`
    NominalConstructor {
        type_name: String,
    },
}

/// xoshiro256++ PRNG state.
#[derive(Debug, Clone)]
pub struct RngState {
    s: [u64; 4],
}

impl RngState {
    pub fn from_seed(seed: u64) -> Self {
        // SplitMix64 to expand seed into 4 state words.
        //
        // The counter advances by the golden gamma and nothing else; the
        // mixing runs on a copy. That's what makes the state a Weyl sequence
        // with a full 2^64 period. Feeding the mixed value back into `z`
        // instead — which this used to do — turns the counter into a chaotic
        // map with no period guarantee, and gave the interpreter a different
        // stream from the C runtime for the same seed.
        let mut z = seed;
        let mut s = [0u64; 4];
        for slot in &mut s {
            z = z.wrapping_add(0x9e3779b97f4a7c15);
            let mut r = z;
            r = (r ^ (r >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            r = (r ^ (r >> 27)).wrapping_mul(0x94d049bb133111eb);
            *slot = r ^ (r >> 31);
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
        items: Arc<Mutex<VecData>>,
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
    /// Owned pre-computed elements (e.g. string lines/chars/split results).
    PreComputed {
        items: std::vec::Vec<Value>,
        index: usize,
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
            Self::PreComputed { index, items } => write!(f, "PreComputedIter({}/{})", index, items.len()),
        }
    }
}

/// A staged `Wide<T>` plan. Built by `.wide()` + adapters, executed by a
/// terminal. Element-wise nodes (`Source`, `Map`, `ZipWith`) produce a lane
/// vector; the CPU executor walks this tree (rask-interp `wide.rs`). Immutable
/// once built — laziness with no hidden execution (conc.data-parallel C1).
#[derive(Debug, Clone)]
pub enum WidePlan {
    /// Lanes materialized from a Vec (`data.wide()`).
    Source(Arc<Mutex<VecData>>),
    /// Apply a closure to each lane.
    Map { source: Arc<WidePlan>, mapper: Value },
    /// Combine two plans lane-by-lane with a closure. Lengths must match.
    ZipWith { a: Arc<WidePlan>, b: Arc<WidePlan>, combiner: Value },
}

impl Value {
    /// An unbounded vector holding `items`.
    pub fn vec(items: Vec<Value>) -> Value {
        Value::Vec(Arc::new(Mutex::new(VecData::new(items))))
    }

    /// CP3: a vector bounded at `n`, pre-allocated.
    pub fn vec_fixed(n: usize) -> Value {
        Value::Vec(Arc::new(Mutex::new(VecData::fixed(n))))
    }

    /// Integer of unknown source width (lengths, indices, internal results).
    /// Unchecked for overflow — use `Value::Int(n, kind)` when the width is
    /// known (from a literal, cast, or typed context).
    pub fn int(n: i64) -> Self {
        Value::Int(n, IntKind::Untyped)
    }

    /// Create a new struct value wrapped in Arc<Mutex<>>.
    pub fn new_struct(name: String, fields: IndexMap<String, Value>, resource_id: Option<u64>) -> Self {
        Value::Struct(Arc::new(Mutex::new(StructData { name, fields, resource_id })))
    }

    /// Create an enum value. variant_index defaults to 0 (builtin enums).
    /// User-defined enums should use `enum_with_index` for correct ordering.
    pub fn enum_val(name: String, variant: String, fields: Vec<Value>) -> Self {
        Value::Enum { name, variant, fields, variant_index: 0, origin: None }
    }

    /// Create an enum value with an explicit variant index for ordering.
    pub fn enum_with_index(name: String, variant: String, fields: Vec<Value>, variant_index: u32) -> Self {
        Value::Enum { name, variant, fields, variant_index, origin: None }
    }

    /// Get the type name for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "void",
            Value::Bool(_) => "bool",
            Value::Int(_, _) => "i64",
            Value::Int128(_) => "i128",
            Value::Uint128(_) => "u128",
            Value::Float(_, _) => "f64",
            Value::Char(_) => "char",
            Value::String(_) => "string",
            Value::Struct(_) => "struct",
            Value::Enum { .. } => "enum",
            Value::Function { .. } => "func",
            Value::Builtin(_) => "builtin",
            Value::Range { .. } => "range",
            Value::Vec(_) => "Vec",
            Value::Wide(_) => "Wide",
            Value::TypeConstructor { .. } => "type",
            Value::EnumConstructor { .. } => "enum constructor",
            Value::Module(_) => "module",
            Value::Package(_) => "package",
            Value::File(_) => "File",
            Value::Closure { .. } => "closure",
            Value::Duration(_) => "Duration",
            Value::Instant(_) => "Instant",
            Value::Type(_) => "type",
            Value::Cell(_) => "Cell",
            Value::Pool(_) => "Pool",
            Value::Store(_) => "Store",
            Value::Link { .. } => "Link",
            Value::Handle { .. } => "Handle",
            Value::WeakHandle { .. } => "WeakHandle",
            Value::ThreadHandle(_) => "ThreadHandle",
            Value::TaskHandle(_) => "TaskHandle",
            Value::TaskGroup(_) => "TaskGroup",
            Value::MultitaskingRuntime(_) => "MultitaskingRuntime",
            Value::Sender(_) => "Sender",
            Value::Receiver(_) => "Receiver",
            Value::ThreadPool(_) => "ThreadPool",
            Value::Map(_) => "Map",
            Value::AtomicBool(_) => "Atomic<bool>",
            Value::AtomicUsize(_) => "Atomic<usize>",
            Value::AtomicU64(_) => "Atomic<u64>",
            Value::Shared(_) => "Shared",
            Value::RaskMutex(_) => "Mutex",
            Value::TcpListener(_) => "TcpListener",
            Value::TcpConnection(_) => "TcpConnection",
            Value::SimdF32x8(_) => "f32x8",
            Value::Rng(_) => "Random",
            Value::StringBuilder(_) => "StringBuilder",
            Value::Iterator(_) => "Iterator",
            Value::Nominal { .. } => "nominal",
            Value::NominalConstructor { .. } => "nominal constructor",
        }
    }

    /// Produce the default value for a type string (DF4).
    pub fn default_for_type(ty: &str) -> Value {
        match ty {
            "i8" | "i16" | "i32" | "i64" | "int" | "isize" |
            "u8" => Value::Int(0, IntKind::U8),
            "u16" => Value::Int(0, IntKind::U16),
            "u32" => Value::Int(0, IntKind::U32),
            "u64" | "uint" => Value::Int(0, IntKind::U64),
            "usize" => Value::Int(0, IntKind::usize_kind()),
            "i128" => Value::Int128(0),
            "u128" => Value::Uint128(0),
            "f32" | "f64" => Value::Float(0.0, FloatKind::Untyped),
            "bool" => Value::Bool(false),
            "char" => Value::Char('\0'),
            "string" => Value::String(Arc::new(Mutex::new(String::new()))),
            "()" => Value::Unit,
            _ => Value::Unit,
        }
    }

    /// Copy a value into a new owner, giving value-type aggregates independent
    /// storage (mem value semantics VS1). A ≤16-byte struct is Copy; binding or
    /// storing it must copy, so mutating the copy can't alias the source.
    ///
    /// Structs, enums, and nominals are copied structurally — their value-type
    /// fields recurse, so nested aggregates are independent too. Reference/box
    /// types (Vec, Map, String, Cell, Shared, Mutex, Pool, handles) keep sharing
    /// their storage: they're move-only, so the source is already dead, and boxes
    /// alias by design. Resource-tracked structs (@resource) are move-only linear
    /// values — never duplicate their storage.
    pub fn copy_on_bind(&self) -> Value {
        match self {
            Value::Struct(s) => {
                let guard = s.lock().unwrap();
                if guard.resource_id.is_some() {
                    // Linear resource: moved, not copied. Keep the shared cell.
                    return Value::Struct(Arc::clone(s));
                }
                let fields: IndexMap<String, Value> = guard.fields.iter()
                    .map(|(k, v)| (k.clone(), v.copy_on_bind()))
                    .collect();
                Value::new_struct(guard.name.clone(), fields, guard.resource_id)
            }
            Value::Enum { name, variant, fields, variant_index, origin } => Value::Enum {
                name: name.clone(),
                variant: variant.clone(),
                fields: fields.iter().map(|f| f.copy_on_bind()).collect(),
                variant_index: *variant_index,
                origin: origin.clone(),
            },
            Value::Nominal { type_name, inner } => Value::Nominal {
                type_name: type_name.clone(),
                inner: Box::new(inner.copy_on_bind()),
            },
            // Reference/box types share; scalars are cheap clones.
            other => other.clone(),
        }
    }

    /// Deep clone a value — creates independent copies of reference-counted internals.
    pub fn deep_clone(&self) -> Value {
        match self {
            Value::String(s) => Value::String(Arc::new(Mutex::new(s.lock().unwrap().clone()))),
            Value::Vec(v) => {
                let deep: Vec<Value> = v.lock().unwrap().iter().map(|val| val.deep_clone()).collect();
                Value::vec(deep)
            }
            Value::Struct(s) => {
                let guard = s.lock().unwrap();
                let deep_fields: IndexMap<String, Value> = guard.fields.iter()
                    .map(|(k, v)| (k.clone(), v.deep_clone()))
                    .collect();
                Value::new_struct(guard.name.clone(), deep_fields, guard.resource_id)
            }
            Value::Enum { name, variant, fields, variant_index, origin } => {
                Value::Enum {
                    name: name.clone(),
                    variant: variant.clone(),
                    fields: fields.iter().map(|f| f.deep_clone()).collect(),
                    variant_index: *variant_index,
                    origin: origin.clone(),
                }
            }
            Value::Cell(c) => {
                let inner = c.lock().unwrap().deep_clone();
                Value::Cell(Arc::new(Mutex::new(inner)))
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
                new_pool.capacity = pool.capacity;
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
                let deep: MapData = map.iter()
                    .map(|(k, v)| (MapKey(k.0.deep_clone()), v.deep_clone()))
                    .collect();
                Value::Map(Arc::new(Mutex::new(deep)))
            }
            Value::RaskMutex(m) => {
                let inner = m.lock().unwrap();
                Value::RaskMutex(Arc::new(std::sync::Mutex::new(inner.deep_clone())))
            }
            // Value types — regular clone is sufficient
            other => other.clone(),
        }
    }

    /// Extract u64 from Value::Int (for Duration constructors).
    pub fn as_int(&self) -> Result<i64, String> {
        match self {
            Value::Int(n, _) => Ok(*n),
            _ => Err(format!("Expected integer, found {}", self.type_name())),
        }
    }

    pub fn as_u64(&self) -> Result<u64, String> {
        match self {
            Value::Int(n, _) if *n >= 0 => Ok(*n as u64),
            Value::Int(n, _) => Err(format!("Cannot convert negative integer {} to u64", n)),
            _ => Err(format!("Expected integer, found {}", self.type_name())),
        }
    }

    /// Extract f64 from Value::Float (for Duration.seconds_f64).
    pub fn as_f64(&self) -> Result<f64, String> {
        match self {
            Value::Float(f, _) => Ok(*f),
            Value::Int(n, _) => Ok(*n as f64),
            _ => Err(format!("Expected float, found {}", self.type_name())),
        }
    }

    /// Get the resource ID if this value is a tracked resource.
    pub fn resource_id(&self) -> Option<u64> {
        match self {
            Value::Struct(s) => s.lock().unwrap().resource_id,
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
            // An unsigned value holds its bit pattern in the i64, so the top
            // half of u64 prints as a negative number unless the width says
            // otherwise (#517).
            Value::Int(n, k) if k.is_unsigned() => write!(f, "{}", *n as u64),
            Value::Int(n, _) => write!(f, "{}", n),
            Value::Int128(n) => write!(f, "{}", n),
            Value::Uint128(n) => write!(f, "{}", n),
            // An f32 prints at f32 width. Widening it to f64 first spells the
            // same number as 0.01666666567325592 instead of 0.016666666.
            Value::Float(n, k) => write!(f, "{}", k.format(*n)),
            Value::Char(c) => write!(f, "{}", c),
            Value::String(s) => write!(f, "{}", s.lock().unwrap()),
            Value::Struct(s) => {
                let guard = s.lock().unwrap();
                write!(f, "{} {{ ", guard.name)?;
                for (i, (k, v)) in guard.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, " }}")
            }
            Value::Enum { name, variant, fields, .. } => {
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
            Value::Range { start, end, inclusive, step, rev } => {
                if *inclusive {
                    write!(f, "{}..={}", start, end)?;
                } else {
                    write!(f, "{}..{}", start, end)?;
                }
                if *step != 1 {
                    write!(f, ".step({})", step)?;
                }
                if *rev {
                    write!(f, ".rev()")?;
                }
                Ok(())
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
            Value::Wide(_) => write!(f, "<Wide plan>"),
            Value::TypeConstructor { kind, type_param } => {
                let base_name = match kind {
                    TypeConstructorKind::Vec => "Vec",
                    TypeConstructorKind::Map => "Map",
                    TypeConstructorKind::String => "string",
                    TypeConstructorKind::Char => "char",
                    TypeConstructorKind::Pool => "Pool",
                    TypeConstructorKind::Store => "Store",
                    TypeConstructorKind::Cell => "Cell",
                    TypeConstructorKind::Channel => "Channel",
                    TypeConstructorKind::Shared => "Shared",
                    TypeConstructorKind::Mutex => "Mutex",
                    TypeConstructorKind::Atomic => "Atomic",
                    TypeConstructorKind::Ordering => "Ordering",
                    TypeConstructorKind::TaskGroup => "TaskGroup",
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
            Value::Module(kind) => write!(f, "<module {}>", kind.name()),
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
            Value::Cell(c) => {
                let inner = c.lock().unwrap();
                write!(f, "Cell({})", inner)
            }
            Value::Pool(p) => {
                let pool = p.lock().unwrap();
                write!(f, "<Pool len={}>", pool.len)
            }
            Value::Store(s) => {
                let store = s.lock().unwrap();
                write!(f, "<Store len={}>", store.len)
            }
            // Print the node, not the address — a link is the node, as far as
            // reading it goes.
            Value::Link { node, .. } => {
                let guard = node.lock().unwrap();
                write!(f, "{}", Value::Struct(Arc::new(Mutex::new(guard.clone()))))
            }
            Value::Handle {
                pool_id,
                index,
                generation,
            } => write!(f, "Handle({}, {}, {})", pool_id, index, generation),
            Value::WeakHandle {
                pool_id,
                index,
                generation,
            } => write!(f, "WeakHandle({}, {}, {})", pool_id, index, generation),
            Value::ThreadHandle(_) => write!(f, "<ThreadHandle>"),
            Value::TaskHandle(_) => write!(f, "<TaskHandle>"),
            Value::TaskGroup(tasks) => write!(f, "<TaskGroup len={}>", tasks.lock().unwrap().len()),
            Value::MultitaskingRuntime(r) => write!(f, "<Multitasking runtime workers={}>", r.workers),
            Value::Sender(_) => write!(f, "<Sender>"),
            Value::Receiver(_) => write!(f, "<Receiver>"),
            Value::ThreadPool(p) => write!(f, "<ThreadPool size={}>", p.size),
            Value::Map(m) => {
                let entries = map_entries_seeded(&m.lock().unwrap());
                write!(f, "Map {{ ")?;
                for (i, (k, v)) in entries.iter().enumerate() {
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
            Value::RaskMutex(m) => {
                let inner = m.lock().unwrap();
                write!(f, "Mutex({})", inner)
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
            Value::Rng(_) => write!(f, "<Random>"),
            Value::StringBuilder(_) => write!(f, "<StringBuilder>"),
            Value::Iterator(_) => write!(f, "<Iterator>"),
            // A nominal newtype's inherited traits delegate to the value it
            // wraps (type.aliases/T12), so rendering one shows the value. The
            // wrapper form printed `Id(42)` where native — which carries no
            // wrapper at runtime — printed `42`.
            Value::Nominal { inner, .. } => write!(f, "{}", inner),
            Value::NominalConstructor { type_name } => write!(f, "<type {}>", type_name),
        }
    }
}

/// How many values a range yields (ctrl.ranges R1/R2/R4, SP1–SP4).
///
/// One formula for every combination of direction, step, and inclusivity:
/// truncating division of the span by the step, rounded up when the span
/// doesn't divide evenly, and floored at zero. A step pointing the wrong way
/// makes the quotient negative, which is exactly the empty range the spec asks
/// for — `(0..10).step(-1)` yields nothing rather than looping forever.
///
/// Codegen builds the same expression in MIR, so the two backends agree by
/// construction rather than by two hand-written loops happening to match.
pub fn range_count(start: i64, end: i64, inclusive: bool, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    let diff = end.wrapping_sub(start);
    let q = diff / step;
    let n = if inclusive {
        q + 1
    } else if q * step != diff {
        q + 1
    } else {
        q
    };
    n.max(0)
}
