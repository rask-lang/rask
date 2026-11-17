// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR operands and rvalues.

use crate::MirType;
use rask_stdlib::mir_metadata::{CallShape, Receiver};

pub use crate::function::LocalId;

/// MIR operand - value that can be used
#[derive(Debug, Clone)]
pub enum MirOperand {
    Local(LocalId),
    Constant(MirConst),
}

/// MIR constant value
#[derive(Debug, Clone)]
pub enum MirConst {
    Int(i64),
    /// A 128-bit constant, already widened correctly for its type: an `i128`
    /// sign-extends from the literal, a `u128` zero-extends. The distinction is
    /// only visible here, where the MIR type is still in hand — Cranelift's
    /// integer types carry no signedness (#762).
    Int128(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
    /// A `bytes` literal. Same 16-byte value and static buffer as a string
    /// literal, without the UTF-8 promise (std.bytes/BY3).
    Bytes(Vec<u8>),
    /// What one element of a container is, for the constructor that builds
    /// it: the element's type with its containers still named
    /// (`MirType::Container`). Codegen turns it into the offsets of what each
    /// element owns; it never becomes a runtime value itself.
    Elem(MirType),
}

impl MirConst {
    /// A string or bytes literal's contents. Both compile to a static buffer
    /// whose count never changes, so neither needs refcounting.
    pub fn static_buffer(&self) -> Option<&[u8]> {
        match self {
            MirConst::String(s) => Some(s.as_bytes()),
            MirConst::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

/// How a field read hands its value back.
///
/// This used to be a bare `Option<u32>` called `field_size`, carrying two
/// different facts depending on which branch of codegen read it: "how many
/// bytes" in one place, "MIR already decided this is an aggregate" in another.
/// The two only stayed apart because a byte-offset check happened to separate
/// them.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldAccess {
    /// No layout info — read a word at `field_index * 8`.
    Word,
    /// The field is this many bytes. It comes back loaded if it fits a
    /// register, and as an address if it doesn't.
    Sized(u32),
    /// The field lives in place — hand back its address whatever its size.
    /// A `T? or E` payload is this: `ok` and `err` can disagree about being an
    /// aggregate, so the size alone can't decide it (#383/#389).
    InPlace(u32),
    /// The field comes back loaded whatever its size — the mirror of
    /// `InPlace`. Only a 128-bit integer needs it: it is sixteen bytes and
    /// still a scalar, held in a register pair, so `Sized`'s "bigger than a
    /// word means an address" read handed back the field's address and
    /// `ledger.balance` printed a stack address (#933).
    InRegister(u32),
}

impl FieldAccess {
    /// How a field of this type comes back. Aggregates live in their own
    /// storage, so the address is the answer even for the small ones — a
    /// fieldless enum is one byte, and loading that byte as a value handed
    /// codegen a tag where it expected a pointer (#561).
    pub fn for_field(ty: &MirType, size: u32) -> FieldAccess {
        // A niche option is one word — the link itself, or the null address
        // for `none` — so it comes back loaded like a link, not addressed like
        // the tagged option its type reads as. Handed back as an address, a
        // `next: Link<Node>?` holding `none` looked present (#1397's debug
        // rendering walked a stack address as a node).
        if ty.niche_none().is_some() {
            FieldAccess::InRegister(size.min(8))
        } else if ty.passed_by_address() {
            FieldAccess::InPlace(size)
        } else {
            // Anything else is a scalar, and a scalar comes back loaded — the
            // size doesn't get a vote. `Sized` reads a size over a word as
            // "aggregate", which is right for the sites that build one without
            // a type to consult and wrong for the one scalar wider than a word.
            FieldAccess::InRegister(size)
        }
    }

    /// Does reading this field yield an address rather than a loaded value?
    pub fn is_address(&self) -> bool {
        match self {
            FieldAccess::Word => false,
            FieldAccess::Sized(size) => *size > 8,
            FieldAccess::InPlace(_) => true,
            FieldAccess::InRegister(_) => false,
        }
    }

    /// The field's size in bytes, when it's known.
    pub fn size(&self) -> Option<u32> {
        match self {
            FieldAccess::Word => None,
            FieldAccess::Sized(size)
            | FieldAccess::InPlace(size)
            | FieldAccess::InRegister(size) => Some(*size),
        }
    }
}

/// MIR rvalue - right-hand side of assignment
#[derive(Debug, Clone)]
pub enum MirRValue {
    Use(MirOperand),
    Ref(LocalId),
    Deref(MirOperand),
    BinaryOp {
        op: BinOp,
        left: MirOperand,
        right: MirOperand,
    },
    UnaryOp {
        op: UnaryOp,
        operand: MirOperand,
    },
    Cast {
        value: MirOperand,
        target_ty: MirType,
    },
    /// The same bits as another scalar of the same width (`transmute`).
    /// Float↔int is a register move, int↔int of one width is nothing at all.
    /// Aggregates never come through here: they go through memory.
    Bitcast {
        value: MirOperand,
        source_ty: MirType,
        target_ty: MirType,
    },
    /// Explicit lossy conversion (type.primitives CV5–CV10). Carries the source
    /// type so codegen has signedness/width without re-deriving it.
    Convert {
        value: MirOperand,
        source_ty: MirType,
        target_ty: MirType,
        kind: rask_ast::expr::ConvertKind,
    },
    Field {
        base: MirOperand,
        field_index: u32,
        /// Pre-computed byte offset from struct layout, when available.
        byte_offset: Option<u32>,
        /// How the field comes back.
        access: FieldAccess,
    },
    EnumTag {
        value: MirOperand,
    },
    /// Load element from a fixed-size array: base_ptr + index * elem_size
    ArrayIndex {
        base: MirOperand,
        index: MirOperand,
        elem_size: u32,
    },
    /// The address of a function, as a bare C function pointer — for the
    /// runtime, which calls it with the C ABI. A closure is `ClosureCreate`.
    FuncAddr(String),
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    /// std.bits B1 — rotation, distinct from Shl/Shr in that bits wrap around
    /// the receiver's width instead of falling off the end.
    RotateLeft,
    RotateRight,
    /// type.integer-overflow OV5/SH2 — the escape hatches from the checked
    /// default. Same arithmetic as Add/Sub/Mul, with the overflow guard
    /// replaced: `Wrapping*` keeps the low bits, `Saturating*` pins to the
    /// receiver type's min or max, and the shift forms mask the amount to the
    /// width instead of trapping on it.
    WrappingAdd,
    WrappingSub,
    WrappingMul,
    WrappingShl,
    WrappingShr,
    SaturatingAdd,
    SaturatingSub,
    SaturatingMul,
    /// Answer the overflow flag itself rather than a number, so `checked_*` and
    /// `overflowing_*` can build their `T?` and `(T, bool)` from it. `Div`'s
    /// flag covers both ways integer division fails: a zero divisor, and the
    /// one signed pair whose quotient doesn't exist (MIN / -1).
    OverflowAdd,
    OverflowSub,
    OverflowMul,
    OverflowDiv,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    // std.bits B1. These map to single machine instructions, so they're MIR
    // ops rather than runtime calls — and because codegen carries the real
    // width per value, each one counts over the receiver's own type rather
    // than whatever register it happens to sit in.
    // `count_zeros`/`leading_ones`/`trailing_ones` compose from these with
    // BitNot instead of getting variants of their own.
    CountOnes,
    LeadingZeros,
    TrailingZeros,
    ReverseBits,
    SwapBytes,
}

/// Function reference for calls
#[derive(Debug, Clone)]
pub struct FunctionRef {
    pub name: String,
    /// True for extern "C" functions — bypasses stdlib dispatch adaptation.
    pub is_extern: bool,
    /// A `.clone()` that runs no code the program wrote, so clone elision may
    /// turn it into a move (comp.clone-elision CE3). Lowering says so from
    /// the receiver's checked type; nothing else may decide it from the name.
    pub elidable_clone: bool,
    /// The call answers a raw pointer, which points into the storage of its
    /// arguments (mem.unsafe/PT1). Lowering says so from the call's checked
    /// type, so liveness keeps those arguments alive while the pointer is.
    pub points_into_args: bool,
    /// A call of a C function-like macro (struct.c-interop/PP4), as the
    /// checker typed it. Not a symbol: a pass after lowering folds it to a
    /// constant or calls a C function made for these argument types.
    pub c_macro: Option<Box<rask_c_parse::cexpr::MacroCall>>,
    /// A C runtime function called by its own symbol, a call lowering makes
    /// for itself (#1555): every argument a word, a word back when the call
    /// has a destination. Codegen declares it from the call, so it needs no
    /// dispatch row.
    pub runtime: bool,
    /// What the call does with its arguments and result, for the ownership
    /// passes. A stdlib method's is read off its declaration when the call is
    /// made; an operation lowering makes up states its own where it emits it
    /// (`borrowing`, `releasing`, `reading`, `like`). `None` is an ordinary function: it
    /// reads its arguments and the passes ask its body, if there is one.
    pub shape: Option<CallShape>,
}

impl FunctionRef {
    /// A call by symbol. A stdlib symbol takes the shape its declaration gives.
    pub fn internal(name: String) -> Self {
        let shape = rask_stdlib::mir_metadata::declared_shape(&name);
        Self { name, is_extern: false, elidable_clone: false, points_into_args: false, c_macro: None, runtime: false, shape }
    }

    /// Extern "C" call.
    pub fn extern_c(name: String) -> Self {
        Self { name, is_extern: true, elidable_clone: false, points_into_args: false, c_macro: None, runtime: false, shape: None }
    }

    /// A runtime function by its C symbol (`runtime`).
    pub fn runtime(name: String) -> Self {
        Self { runtime: true, ..Self::internal(name) }
    }

    /// A call whose shape lowering states, because no declaration answers for it.
    pub fn shaped(name: impl Into<String>, shape: CallShape) -> Self {
        Self { shape: Some(shape), ..Self::internal(name.into()) }
    }

    /// A spelling lowering makes up for the stdlib operation `declared_as`
    /// (`Vec_index` for `Vec.get`, a `Shared<T, Local>`'s `Cell_set` for
    /// `Shared.set`): the same operation, so the same shape.
    pub fn like(name: impl Into<String>, declared_as: &str) -> Self {
        let shape = rask_stdlib::mir_metadata::declared_shape(declared_as)
            .unwrap_or_else(|| panic!("`{declared_as}` is declared nowhere in stdlib/*.rk"));
        Self::shaped(name, shape)
    }

    /// An operation that borrows its receiver, keeps nothing and hands back
    /// something fresh.
    pub fn borrowing(name: impl Into<String>) -> Self {
        Self::shaped(name, CallShape::borrows_receiver())
    }

    /// A free or a release: the call is the receiver's last use.
    pub fn releasing(name: impl Into<String>) -> Self {
        Self::shaped(name, CallShape::consumes_receiver())
    }

    /// A constructor or other call with no receiver that only reads its
    /// arguments.
    pub fn reading(name: impl Into<String>) -> Self {
        Self::shaped(name, CallShape::no_receiver())
    }

    /// A call with no receiver that keeps or frees the arguments at these
    /// positions and only reads the rest.
    pub fn keeping(name: impl Into<String>, positions: &[usize]) -> Self {
        Self::shaped(name, CallShape::giving(positions))
    }

    /// This call, handing back something made for it that `release` frees.
    pub fn handing_back(self, release: &'static str) -> Self {
        let shape = self.shape.unwrap_or_else(CallShape::no_receiver).handing_back(release);
        Self { shape: Some(shape), ..self }
    }

    /// Does the call keep the argument at `index`, as far as its shape says?
    pub fn gives(&self, index: usize) -> bool {
        self.shape.is_some_and(|s| s.gives(index))
    }

    /// Does the call borrow its receiver rather than consume it?
    pub fn borrows_receiver(&self) -> bool {
        self.shape.is_some_and(|s| s.receiver() == Receiver::Borrowed)
    }

    /// Does the call take its receiver away: `take self`, or a free?
    pub fn consumes_receiver(&self) -> bool {
        self.shape.is_some_and(|s| s.receiver() == Receiver::Consumed)
    }

    /// Does the call hand back an element it took out of its receiver?
    pub fn moves_out(&self) -> bool {
        self.shape.is_some_and(|s| s.moves_out())
    }

    /// Does the result point into storage an argument keeps owning?
    pub fn returns_a_view(&self) -> bool {
        self.shape.is_some_and(|s| s.returns_a_view())
    }

    /// Which argument the result points into, receiver first.
    pub fn viewed_argument(&self) -> Option<usize> {
        self.shape.and_then(|s| s.viewed_argument())
    }
}

/// A spelling lowering makes up, held in a table until the call is emitted,
/// with what the call it names does. The name and the fact are written side by
/// side where the table is, so neither can be read from the other.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mint {
    name: &'static str,
    what: MintWhat,
}

#[derive(Clone, Copy, Debug)]
enum MintWhat {
    /// The stdlib operation it realises.
    Like(&'static str),
    Borrowing,
}

impl Mint {
    pub(crate) const fn like(name: &'static str, declared_as: &'static str) -> Self {
        Self { name, what: MintWhat::Like(declared_as) }
    }

    pub(crate) const fn borrowing(name: &'static str) -> Self {
        Self { name, what: MintWhat::Borrowing }
    }

    pub(crate) fn call(self) -> FunctionRef {
        match self.what {
            MintWhat::Like(declared_as) => FunctionRef::like(self.name, declared_as),
            MintWhat::Borrowing => FunctionRef::borrowing(self.name),
        }
    }
}
