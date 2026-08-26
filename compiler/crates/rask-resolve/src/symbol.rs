// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Symbol definitions and symbol table.

use rask_ast::Span;
use rask_ast::decl::ContextClause;
use crate::package::PackageId;

/// Unique identifier for a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(pub u32);

/// The kind of symbol.
#[derive(Debug, Clone)]
pub enum SymbolKind {
    /// A local variable binding.
    Variable {
        /// Whether this binding is mutable (let vs const).
        mutable: bool,
    },
    /// A function parameter.
    Parameter {
        /// Whether this parameter takes ownership.
        is_take: bool,
        /// Whether this parameter is mutable.
        is_mutate: bool,
        is_deleting: bool,
    },
    /// A function.
    Function {
        /// SymbolIds of parameters.
        params: Vec<SymbolId>,
        /// Return type as a string (for now).
        ret_ty: Option<String>,
        /// `using` context clauses.
        context_clauses: Vec<ContextClause>,
        /// Whether this is an `unsafe func`.
        is_unsafe: bool,
    },
    /// An extern function (C FFI).
    ExternFunction {
        /// ABI string (e.g., "C").
        abi: String,
        /// Parameter type strings.
        params: Vec<String>,
        /// Return type as a string (None = void).
        ret_ty: Option<String>,
    },
    /// A struct type.
    Struct {
        /// Field names and their SymbolIds.
        fields: Vec<(String, SymbolId)>,
    },
    /// An enum type.
    Enum {
        /// Variant names and their SymbolIds.
        variants: Vec<(String, SymbolId)>,
    },
    /// An enum variant.
    EnumVariant {
        /// The enum this variant belongs to.
        enum_id: SymbolId,
    },
    /// A trait.
    Trait {
        /// Method SymbolIds.
        methods: Vec<SymbolId>,
        /// Super-trait names.
        super_traits: Vec<String>,
    },
    /// A struct field.
    Field {
        /// The struct this field belongs to.
        parent: SymbolId,
    },
    /// A built-in type (Vec, Map, string, etc.).
    BuiltinType {
        /// The built-in type kind.
        builtin: BuiltinTypeKind,
    },
    /// A built-in function (println, panic, etc.).
    BuiltinFunction {
        /// The built-in function kind.
        builtin: BuiltinFunctionKind,
    },
    /// A built-in module (io, fs, env, etc.).
    BuiltinModule {
        /// The built-in module kind.
        module: BuiltinModuleKind,
    },
    /// An external package namespace (for `import pkg` where pkg is a real package).
    ExternalPackage {
        /// The PackageId this namespace refers to.
        package_id: PackageId,
    },
    /// A type alias (transparent).
    TypeAlias {
        /// The target type name.
        target: String,
    },
    /// A C import namespace (`import c "header.h"` → `c.symbol`).
    CNamespace {
        /// Symbols parsed from C headers, keyed by name.
        members: std::collections::HashMap<String, SymbolId>,
    },
}

/// Built-in type kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinTypeKind {
    /// Vec<T> - dynamic array
    Vec,
    /// Map<K, V> - hash map
    Map,
    /// Set<T> - hash set
    Set,
    /// string - UTF-8 string
    String,
    /// Error - error type
    Error,
    /// Channel<T> - message channel
    Channel,
    /// Pool<T> - arena allocator for graph structures
    Pool,
    /// Cell<T> - single heap-allocated mutable value (CE1-CE6)
    Cell,
    /// Handle<T> - typed reference into a Pool<T>
    Handle,
    /// Rack<T> - arena whose incoming edges are fixed at delete
    Rack,
    /// Link<T> - one edge to a node in a Rack<T>
    Link,
    /// Atomic<T> - atomic operations
    Atomic,
    /// Shared<T> - shared state with interior mutability
    Shared,
    /// Mutex<T> - mutual exclusion lock
    Mutex,
    /// Heap<T> - the one-pointer indirection (mem.heap)
    Heap,
    /// SIMD vector types (f32x4, f32x8, i32x4, i32x8, f64x2, f64x4)
    Simd,
    /// Rng - random number generator
    Rng,
    /// File - file handle
    File,
    /// Primitive numeric/bool/char types (u8, i32, f64, bool, char, etc.)
    Primitive,
}

/// Built-in function kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFunctionKind {
    /// println - print with newline
    Println,
    /// print - print without newline
    Print,
    /// panic - abort with message
    Panic,
    /// format - string formatting
    Format,
    /// spawn - spawn a concurrent task
    Spawn,
    /// transmute - reinterpret bits as different type (unsafe)
    Transmute,
    /// todo - panic with "not yet implemented"
    Todo,
    /// unreachable - panic with "entered unreachable code"
    Unreachable,
    /// min - generic minimum of two comparable values
    Min,
    /// max - generic maximum of two comparable values
    Max,
    /// clamp - constrain value between lo and hi
    Clamp,
    /// assert_eq - compare got/expected with pretty-print diff
    AssertEq,
    /// skip - skip rest of test with reason
    Skip,
    /// expect_fail - invert pass/fail for test
    ExpectFail,
    /// drop - consume a `Heap<T>`, freeing what it points at
    /// (mem.owned/OW3)
    Drop,
}

/// A builtin type and the name it's in scope under.
///
/// `register_builtins` walks this to put them in scope, and `is_builtin_type`
/// reads the same table to answer BI3. Asking the table by name rather than
/// asking the scope what it holds is the point: the stdlib declares
/// `public struct Vec<T> { }` of its own, and that binding replaced the builtin
/// one in scope, so `struct Vec { … }` in a program was accepted while
/// `struct Set { … }` was refused — the difference being only whether the stdlib
/// happened to declare the name too (#977).
pub struct BuiltinTypeEntry {
    pub name: &'static str,
    pub kind: BuiltinTypeKind,
    /// The module that brings this name into scope, or `None` for BI1's
    /// always-available set.
    pub module: Option<&'static str>,
}

const fn always(name: &'static str, kind: BuiltinTypeKind) -> BuiltinTypeEntry {
    BuiltinTypeEntry { name, kind, module: None }
}

const fn from(module: &'static str, name: &'static str, kind: BuiltinTypeKind) -> BuiltinTypeEntry {
    BuiltinTypeEntry { name, kind, module: Some(module) }
}

/// Every type the compiler provides, and where it comes from.
///
/// BI1's set is the `always` half: primitives (handled separately, by
/// `rask_ast::primitives`), `string`, `Vec`, `Map`, `Set`, `Error`, `Channel`,
/// `none`. Those are in scope with no import and BI3 reserves their names.
///
/// Everything else needs its module. The box family is compiler-provided and
/// closed (mem.boxes/BX1–BX4), but that's about who may *define* one, not about
/// who can see the name without asking: `Pool`, `Handle`, `Rack`, `Link` and
/// `Heap` live in `memory`, `Shared`, `Mutex` and the atomics in `sync`, and
/// they're imported like anything else. All of them used to be in the always
/// half, so a program couldn't declare a `struct Handle` of its own and
/// `Pool.new()` worked with no import at all (#977).
pub const BUILTIN_TYPES: &[BuiltinTypeEntry] = &[
    always("Vec", BuiltinTypeKind::Vec),
    always("Map", BuiltinTypeKind::Map),
    always("Set", BuiltinTypeKind::Set),
    always("string", BuiltinTypeKind::String),
    always("Error", BuiltinTypeKind::Error),
    always("Channel", BuiltinTypeKind::Channel),

    from("memory", "Pool", BuiltinTypeKind::Pool),
    from("memory", "Handle", BuiltinTypeKind::Handle),
    from("memory", "Rack", BuiltinTypeKind::Rack),
    from("memory", "Link", BuiltinTypeKind::Link),
    from("memory", "Heap", BuiltinTypeKind::Heap),

    from("sync", "Shared", BuiltinTypeKind::Shared),
    from("sync", "Mutex", BuiltinTypeKind::Mutex),
    from("sync", "Atomic", BuiltinTypeKind::Atomic),
    from("sync", "AtomicBool", BuiltinTypeKind::Atomic),
    from("sync", "AtomicI8", BuiltinTypeKind::Atomic),
    from("sync", "AtomicU8", BuiltinTypeKind::Atomic),
    from("sync", "AtomicI16", BuiltinTypeKind::Atomic),
    from("sync", "AtomicU16", BuiltinTypeKind::Atomic),
    from("sync", "AtomicI32", BuiltinTypeKind::Atomic),
    from("sync", "AtomicU32", BuiltinTypeKind::Atomic),
    from("sync", "AtomicI64", BuiltinTypeKind::Atomic),
    from("sync", "AtomicU64", BuiltinTypeKind::Atomic),
    from("sync", "AtomicUsize", BuiltinTypeKind::Atomic),
    from("sync", "AtomicIsize", BuiltinTypeKind::Atomic),

    // These three were a second table, keyed by module in the resolver. Same
    // question, so the same table answers it.
    from("fs", "File", BuiltinTypeKind::File),
    from("random", "Random", BuiltinTypeKind::Rng),
    from("math", "f32x4", BuiltinTypeKind::Simd),
    from("math", "f32x8", BuiltinTypeKind::Simd),
    from("math", "f64x2", BuiltinTypeKind::Simd),
    from("math", "f64x4", BuiltinTypeKind::Simd),
    from("math", "i32x4", BuiltinTypeKind::Simd),
    from("math", "i32x8", BuiltinTypeKind::Simd),
];

/// BI1's set — in scope with no import, and reserved against redeclaration.
pub fn is_builtin_type(name: &str) -> bool {
    rask_ast::primitives::is_scalar(name)
        || BUILTIN_TYPES
            .iter()
            .any(|t| t.name == name && t.module.is_none())
}

/// The builtin type `module` brings into scope under `name`, if any.
pub fn module_builtin_type(module: &str, name: &str) -> Option<BuiltinTypeKind> {
    BUILTIN_TYPES
        .iter()
        .find(|t| t.name == name && t.module == Some(module))
        .map(|t| t.kind)
}

/// The compiler-provided types `module` brings into scope.
pub fn module_builtin_types(module: &str) -> impl Iterator<Item = &'static BuiltinTypeEntry> + use<'_> {
    BUILTIN_TYPES.iter().filter(move |t| t.module == Some(module))
}

/// A builtin function, the name it's in scope under, and whether a program may
/// declare its own.
pub struct BuiltinFnEntry {
    pub name: &'static str,
    pub kind: BuiltinFunctionKind,
    /// Return type as the resolver records it — `"!"` for the diverging ones.
    pub ret_ty: Option<&'static str>,
    /// BF3 refuses a program's own declaration of this name.
    ///
    /// True for BF1's eight, which the compiler knows the signatures of and
    /// generates code for per call site (BF2) — a program's own `println` would
    /// be silently ignored at every interpolation. `min`, `max`, `clamp` and the
    /// test builtins aren't in BF1: they're ordinary generic functions and a
    /// program defining its own has always been allowed.
    pub reserved: bool,
}

const fn bf(
    name: &'static str,
    kind: BuiltinFunctionKind,
    ret_ty: Option<&'static str>,
    reserved: bool,
) -> BuiltinFnEntry {
    BuiltinFnEntry { name, kind, ret_ty, reserved }
}

/// Functions in scope with no import. The `reserved` ones are BF1's, which BF3
/// won't let a program redeclare.
pub const BUILTIN_FUNCTIONS: &[BuiltinFnEntry] = &[
    bf("println", BuiltinFunctionKind::Println, None, true),
    bf("print", BuiltinFunctionKind::Print, None, true),
    bf("panic", BuiltinFunctionKind::Panic, Some("!"), true),
    bf("format", BuiltinFunctionKind::Format, None, true),
    bf("todo", BuiltinFunctionKind::Todo, Some("!"), true),
    bf("unreachable", BuiltinFunctionKind::Unreachable, Some("!"), true),
    bf("transmute", BuiltinFunctionKind::Transmute, None, true),
    // `spawn` is BF1's eighth. It's registered by `async`'s companions rather
    // than here, because `spawn(|| …)` needs `using Multitasking` in scope.
    bf("min", BuiltinFunctionKind::Min, None, false),
    bf("max", BuiltinFunctionKind::Max, None, false),
    bf("clamp", BuiltinFunctionKind::Clamp, None, false),
    bf("assert_eq", BuiltinFunctionKind::AssertEq, None, false),
    bf("skip", BuiltinFunctionKind::Skip, Some("!"), false),
    bf("expect_fail", BuiltinFunctionKind::ExpectFail, None, false),
    bf("drop", BuiltinFunctionKind::Drop, None, false),
];

/// BF1's set — the names BF3 reserves.
pub fn is_reserved_builtin_fn(name: &str) -> bool {
    name == "spawn"
        || BUILTIN_FUNCTIONS.iter().any(|f| f.name == name && f.reserved)
}

/// Enums the resolver puts in scope itself, with no import.
///
/// `Option` and `Result` back `T?` and `T or E`; `Ordering` is what `compare()`
/// answers with. A module's own enums (`Method`, `JsonValue`) are not here —
/// those arrive with an import and a program is free to name a type after one it
/// hasn't imported.
pub const PRELUDE_ENUMS: &[&str] = &["Option", "Result", "Ordering"];

/// Names in scope with no import: BI1's types, BF1's functions, the prelude
/// enums, the primitives.
///
/// This is also exactly what BI3 and BF3 reserve against a program's own
/// declaration, which isn't a coincidence — a name that's always there is a
/// name no declaration can have, and a name that isn't is a name IM1 makes the
/// program ask for. Both rules read this one answer.
///
/// Asked by name, not by looking the name up in scope. The stdlib declares
/// `public struct Vec<T> { }` and `public enum Option<T> { }` of its own, and
/// those bindings replaced the builtin ones — which is why `struct Vec { … }`
/// and `struct Option { … }` were accepted while `struct Set { … }` and
/// `struct Ordering { … }` were refused (#977).
pub fn is_always_in_scope(name: &str) -> bool {
    is_builtin_type(name)
        || is_reserved_builtin_fn(name)
        || PRELUDE_ENUMS.contains(&name)
}

/// A stdlib module, identified by the name it's imported under.
///
/// This was a hand-written enum, and it listed 17 of the stdlib's 29 files —
/// `import memory`, `import string`, `import sync` and eleven others answered
/// "unknown package: `memory`" while the types inside them resolved with no
/// import at all (#977). The set now comes from
/// `rask_stdlib::modules::module_names()`, which reads the stub sources, so
/// there's one list and it can't drift from the stdlib it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuiltinModuleKind(&'static str);

impl BuiltinModuleKind {
    /// `async` — `spawn` and friends come into scope with it.
    pub const ASYNC: Self = Self("async");
    /// `core` — `transmute`.
    pub const CORE: Self = Self("core");
    /// `fs` — carries the builtin `File`.
    pub const FS: Self = Self("fs");
    /// `random` — carries the builtin `Random`.
    pub const RANDOM: Self = Self("random");
    /// `math` — carries the SIMD vector types.
    pub const MATH: Self = Self("math");
    /// `os` — `Output`'s fields are known to the resolver.
    pub const OS: Self = Self("os");

    /// The name this module is imported under. Also the stem of its stdlib
    /// file, which is what `rask_stdlib::modules` keys its exports by.
    pub fn name(self) -> &'static str {
        self.0
    }

    /// The module a name imports, if it's a stdlib module.
    pub fn from_name(name: &str) -> Option<BuiltinModuleKind> {
        rask_stdlib::modules::module_names()
            .iter()
            .copied()
            .find(|m| *m == name)
            .map(BuiltinModuleKind)
    }
}

/// A declared symbol.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    /// Type annotation, if any.
    pub ty: Option<String>,
    /// Where this symbol was declared.
    pub span: Span,
    /// Whether this symbol is public.
    pub is_pub: bool,
}

/// Table of all symbols in a program.
#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self { symbols: Vec::new() }
    }

    /// Insert a new symbol and return its ID.
    pub fn insert(&mut self, name: String, kind: SymbolKind, ty: Option<String>, span: Span, is_pub: bool) -> SymbolId {
        let id = SymbolId(self.symbols.len() as u32);
        self.symbols.push(Symbol {
            id,
            name,
            kind,
            ty,
            span,
            is_pub,
        });
        id
    }

    /// Get a symbol by ID.
    pub fn get(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id.0 as usize)
    }

    /// Get a mutable reference to a symbol by ID.
    pub fn get_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.symbols.get_mut(id.0 as usize)
    }

    /// Iterate over all symbols.
    pub fn iter(&self) -> impl Iterator<Item = &Symbol> {
        self.symbols.iter()
    }
}
