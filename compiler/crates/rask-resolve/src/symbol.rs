//! Symbol definitions and symbol table.

use rask_ast::Span;

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
    },
    /// A function.
    Function {
        /// SymbolIds of parameters.
        params: Vec<SymbolId>,
        /// Return type as a string (for now).
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
    },
    /// A struct field.
    Field {
        /// The struct this field belongs to.
        parent: SymbolId,
    },
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
