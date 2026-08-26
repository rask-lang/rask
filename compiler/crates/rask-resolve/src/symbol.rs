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
