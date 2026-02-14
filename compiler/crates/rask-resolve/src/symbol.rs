// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Symbol definitions and symbol table.

use rask_ast::Span;
use rask_ast::decl::ContextClause;

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
    },
    /// A function.
    Function {
        /// SymbolIds of parameters.
        params: Vec<SymbolId>,
        /// Return type as a string (for now).
        ret_ty: Option<String>,
        /// `using` context clauses.
        context_clauses: Vec<ContextClause>,
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
    /// Atomic<T> - atomic operations
    Atomic,
    /// Shared<T> - shared state with interior mutability
    Shared,
    /// Owned<T> - heap-allocated owned value
    Owned,
    /// f32x8 - SIMD vector type (8 x f32)
    F32x8,
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
}

/// Built-in module kinds (stdlib modules).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinModuleKind {
    /// io - standard input/output
    Io,
    /// fs - filesystem operations
    Fs,
    /// env - environment variables
    Env,
    /// cli - command line arguments
    Cli,
    /// std - standard library utilities
    Std,
    /// json - JSON parsing and encoding
    Json,
    /// random - random number generation
    Random,
    /// time - time and duration utilities
    Time,
    /// math - mathematical functions
    Math,
    /// path - path manipulation
    Path,
    /// os - operating system utilities
    Os,
    /// net - networking
    Net,
    /// core - core utilities and constants
    Core,
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
