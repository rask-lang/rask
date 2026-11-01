// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! C arithmetic expressions with C's types: the body of a `#define`, a
//! function-like macro called with values, an enumerator, an array size.
//!
//! An expression is parsed once into a tree whose leaves are parameters or
//! values with their C types. That tree can then be typed for given parameter
//! types (integer promotion and the usual arithmetic conversions, as C does
//! them), evaluated when every parameter has a value, or written back out as C
//! for a function that computes it at run time. Constant `#define`s, enumerator
//! values and array sizes go through it with no parameters (`consteval`).
//!
//! What isn't arithmetic — a member access, a function call, a pointer, an
//! assignment, `sizeof` — is an error that says which, never a guess.

use crate::{CTokenKind, CType};

/// A value with its C type.
#[derive(Debug, Clone, PartialEq)]
pub enum CVal {
    Int(i128, CType),
    /// `float` or `double`.
    Float(f64, CType),
    Str(String),
}

/// What the parser asks of its caller about names.
pub trait Names {
    /// The value of a name: an object-like macro, an enumerator.
    fn value(&self, name: &str) -> Option<CVal>;
    /// The arithmetic type a typedef name stands for, for casts.
    fn arith_type(&self, name: &str) -> Option<CType>;
    /// A function-like macro's parameters and body, to expand at a call.
    fn function_macro(&self, name: &str) -> Option<(Vec<String>, Vec<CTokenKind>)>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum CExpr {
    Param(usize),
    Lit(CVal),
    Unary(UnOp, Box<CExpr>),
    Binary(BinOp, Box<CExpr>, Box<CExpr>),
    Ternary(Box<CExpr>, Box<CExpr>, Box<CExpr>),
    Cast(CType, Box<CExpr>),
    /// `base->field` (`arrow`) or `base.field`.
    Member { base: Box<CExpr>, field: String, arrow: bool },
    /// A call of a function the header declares, or a compiler builtin.
    Call { func: String, args: Vec<CExpr> },
}

/// What typing a macro that reads memory asks of the header it came from.
pub trait Header {
    /// The type of `field` in the struct or union `record` names.
    fn field(&self, record: &CType, field: &str) -> Option<CType>;
    /// A function's parameter types, result type, and whether it's variadic.
    fn function(&self, name: &str) -> Option<(Vec<CType>, CType, bool)>;
}

/// No header: arithmetic only.
pub struct NoHeader;

impl Header for NoHeader {
    fn field(&self, _: &CType, _: &str) -> Option<CType> {
        None
    }
    fn function(&self, _: &str) -> Option<(Vec<CType>, CType, bool)> {
        None
    }
}

/// The compiler builtins a header's macros reach for, by name.
fn builtin(name: &str) -> Option<(Vec<CType>, CType, bool)> {
    let one = |t: CType| Some((vec![t.clone()], t, false));
    match name {
        "__builtin_bswap16" => one(CType::UnsignedShort),
        "__builtin_bswap32" => one(CType::UnsignedInt),
        "__builtin_bswap64" => one(CType::UnsignedLong),
        _ => None,
    }
}

/// Does evaluating `e` read memory or call into C? Such a macro is computed
/// by C with its header, and a call to it is `unsafe`.
pub fn touches_memory(e: &CExpr) -> bool {
    match e {
        CExpr::Param(_) | CExpr::Lit(_) => false,
        CExpr::Member { .. } | CExpr::Call { .. } => true,
        CExpr::Unary(_, x) | CExpr::Cast(_, x) => touches_memory(x),
        CExpr::Binary(_, a, b) => touches_memory(a) || touches_memory(b),
        CExpr::Ternary(c, a, b) => touches_memory(c) || touches_memory(a) || touches_memory(b),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Plus,
    BitNot,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    OrOr, AndAnd, BitOr, BitXor, BitAnd, Eq, Ne, Lt, Gt, Le, Ge, Shl, Shr, Add, Sub, Mul, Div, Rem,
}

// ---------------------------------------------------------------------------
// Types

/// The bits and signedness of an integer type.
pub fn int_shape(ty: &CType) -> Option<(u32, bool)> {
    Some(match ty {
        CType::Bool => (8, false),
        CType::Char | CType::SignedChar => (8, true),
        CType::UnsignedChar => (8, false),
        CType::Short => (16, true),
        CType::UnsignedShort => (16, false),
        CType::Int => (32, true),
        CType::UnsignedInt => (32, false),
        CType::Long | CType::LongLong | CType::SSizeT => (64, true),
        CType::UnsignedLong | CType::UnsignedLongLong | CType::SizeT => (64, false),
        CType::FixedInt { bits, signed } => (*bits as u32, *signed),
        CType::IntPtr { signed } => (64, *signed),
        CType::Const(inner) => return int_shape(inner),
        _ => return None,
    })
}

fn is_float(ty: &CType) -> bool {
    matches!(canonical(ty), CType::Float | CType::Double)
}

/// The type arithmetic sees: a fixed-width or `size_t` name as the standard
/// type of its width (64 bits is `long`, LP64's choice), qualifiers gone.
pub fn canonical(ty: &CType) -> CType {
    match ty {
        CType::Const(inner) => canonical(inner),
        CType::FixedInt { bits, signed } => match (bits, signed) {
            (8, true) => CType::SignedChar,
            (8, false) => CType::UnsignedChar,
            (16, true) => CType::Short,
            (16, false) => CType::UnsignedShort,
            (32, true) => CType::Int,
            (32, false) => CType::UnsignedInt,
            (_, true) => CType::Long,
            (_, false) => CType::UnsignedLong,
        },
        CType::SizeT => CType::UnsignedLong,
        CType::SSizeT => CType::Long,
        CType::IntPtr { signed: true } => CType::Long,
        CType::IntPtr { signed: false } => CType::UnsignedLong,
        other => other.clone(),
    }
}

fn rank(ty: &CType) -> u32 {
    match canonical(ty) {
        CType::Bool => 0,
        CType::Char | CType::SignedChar | CType::UnsignedChar => 1,
        CType::Short | CType::UnsignedShort => 2,
        CType::Int | CType::UnsignedInt => 3,
        CType::Long | CType::UnsignedLong => 4,
        _ => 5,
    }
}

/// Integer promotion: anything narrower than `int` becomes `int`.
pub fn promote(ty: &CType) -> CType {
    let ty = canonical(ty);
    if !is_float(&ty) && rank(&ty) < 3 { CType::Int } else { ty }
}

fn unsigned_of(ty: &CType) -> CType {
    match canonical(ty) {
        CType::Int => CType::UnsignedInt,
        CType::Long => CType::UnsignedLong,
        CType::LongLong => CType::UnsignedLongLong,
        other => other,
    }
}

/// The usual arithmetic conversions: the type both operands of `a op b` are
/// brought to.
pub fn usual(a: &CType, b: &CType) -> CType {
    let (a, b) = (canonical(a), canonical(b));
    if a == CType::Double || b == CType::Double {
        return CType::Double;
    }
    if a == CType::Float || b == CType::Float {
        return CType::Float;
    }
    let (a, b) = (promote(&a), promote(&b));
    if a == b {
        return a;
    }
    let (abits, asigned) = int_shape(&a).unwrap_or((32, true));
    let (bbits, bsigned) = int_shape(&b).unwrap_or((32, true));
    if asigned == bsigned {
        return if rank(&a) >= rank(&b) { a } else { b };
    }
    let (u, ubits, s, sbits) = if asigned { (b, bbits, a, abits) } else { (a, abits, b, bbits) };
    if rank(&u) >= rank(&s) {
        u
    } else if sbits > ubits {
        s
    } else {
        unsigned_of(&s)
    }
}

/// Whether C takes a `from` for a parameter of type `to` without a cast:
/// a number for a number, a pointer for a pointer to the same thing or to
/// `void`, and a literal 0 for any pointer.
fn passes(from: &CType, to: &CType, null: bool) -> bool {
    let number = |t: &CType| int_shape(t).is_some() || is_float(t);
    let chars = |t: &CType| matches!(t, CType::Char | CType::SignedChar | CType::UnsignedChar);
    match (canonical(from), canonical(to)) {
        (f, t) if number(&f) && number(&t) => true,
        (f, CType::Pointer(_)) if null && int_shape(&f).is_some() => true,
        (CType::Pointer(f), CType::Pointer(t)) => {
            let (f, t) = (canonical(&f), canonical(&t));
            f == CType::Void || t == CType::Void || f == t || (chars(&f) && chars(&t))
        }
        _ => false,
    }
}

/// The type of `e` when its parameters have `params`' types.
pub fn type_of(e: &CExpr, params: &[CType]) -> Result<CType, String> {
    type_of_in(e, params, &NoHeader)
}

/// `type_of`, with the header's structs and functions for a macro that reads
/// memory or calls into C.
pub fn type_of_in(e: &CExpr, params: &[CType], header: &dyn Header) -> Result<CType, String> {
    let int_only = |t: &CType, what: &str| {
        if is_float(t) { Err(format!("`{}` on a floating value", what)) } else { Ok(()) }
    };
    // An operand of anything but a bare literal has to be a number: a string
    // is a pointer in C, and pointer arithmetic isn't something to copy.
    let arith = |e: &CExpr| -> Result<CType, String> {
        let t = type_of_in(e, params, header)?;
        if int_shape(&t).is_none() && !is_float(&t) {
            return Err("arithmetic on a string or a pointer".to_string());
        }
        Ok(t)
    };
    Ok(match e {
        CExpr::Param(i) => canonical(params.get(*i).ok_or("a parameter without an argument")?),
        CExpr::Lit(CVal::Int(_, t)) | CExpr::Lit(CVal::Float(_, t)) => t.clone(),
        CExpr::Lit(CVal::Str(_)) => CType::Pointer(Box::new(CType::Char)),
        CExpr::Cast(t, inner) => {
            arith(inner)?;
            t.clone()
        }
        CExpr::Unary(UnOp::Not, inner) => {
            arith(inner)?;
            CType::Int
        }
        CExpr::Unary(op, inner) => {
            let t = arith(inner)?;
            if *op == UnOp::BitNot {
                int_only(&t, "~")?;
            }
            promote(&t)
        }
        CExpr::Binary(op, a, b) => {
            let (ta, tb) = (arith(a)?, arith(b)?);
            match op {
                BinOp::OrOr | BinOp::AndAnd | BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le
                | BinOp::Ge => CType::Int,
                BinOp::Shl | BinOp::Shr => {
                    int_only(&ta, "a shift")?;
                    int_only(&tb, "a shift")?;
                    promote(&ta)
                }
                BinOp::Rem | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                    int_only(&ta, op_text(*op))?;
                    int_only(&tb, op_text(*op))?;
                    usual(&ta, &tb)
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => usual(&ta, &tb),
            }
        }
        CExpr::Ternary(c, a, b) => {
            arith(c)?;
            usual(&arith(a)?, &arith(b)?)
        }
        CExpr::Member { base, field, arrow } => {
            let t = type_of_in(base, params, header)?;
            let record = match (canonical(&t), arrow) {
                (CType::Pointer(inner), true) => *inner,
                (other, false) => other,
                (_, true) => return Err(format!("`->{}` on something that isn't a pointer", field)),
            };
            header
                .field(&canonical(&record), field)
                .ok_or_else(|| format!("`{}`, which the struct it's read from doesn't have", field))?
        }
        CExpr::Call { func, args } => {
            let (ps, ret, variadic) = builtin(func)
                .or_else(|| header.function(func))
                .ok_or_else(|| format!("a call to `{}`, which the header doesn't declare", func))?;
            if args.len() < ps.len() || (!variadic && args.len() > ps.len()) {
                return Err(format!("a call to `{}` with the wrong number of arguments", func));
            }
            for (i, a) in args.iter().enumerate() {
                let t = type_of_in(a, params, header)?;
                // Past the named parameters, C's default promotions apply.
                let Some(p) = ps.get(i) else { continue };
                let null = matches!(a, CExpr::Lit(CVal::Int(0, _)));
                if !passes(&t, p, null) {
                    return Err(format!(
                        "a call to `{}` handing its argument {} a `{}` where it takes a `{}`",
                        func,
                        i + 1,
                        describe_type(&t),
                        describe_type(p)
                    ));
                }
            }
            if ret == CType::Void {
                return Err(format!("a call to `{}`, which answers nothing", func));
            }
            ret
        }
    })
}

// ---------------------------------------------------------------------------
// Evaluation

/// `v` brought to `ty`, wrapping or truncating as C does.
pub fn convert(v: &CVal, ty: &CType) -> Option<CVal> {
    let ty = canonical(ty);
    if is_float(&ty) {
        let f = match v {
            CVal::Int(i, _) => *i as f64,
            CVal::Float(f, _) => *f,
            CVal::Str(_) => return None,
        };
        let f = if ty == CType::Float { f as f32 as f64 } else { f };
        return Some(CVal::Float(f, ty));
    }
    let (bits, signed) = int_shape(&ty)?;
    let i = match v {
        CVal::Int(i, _) => *i,
        CVal::Float(f, _) => {
            if !f.is_finite() {
                return None;
            }
            f.trunc() as i128
        }
        CVal::Str(_) => return None,
    };
    if ty == CType::Bool {
        return Some(CVal::Int((i != 0) as i128, ty));
    }
    let mask: i128 = (1i128 << bits) - 1;
    let u = i & mask;
    let out = if signed && (u >> (bits - 1)) & 1 == 1 { u - (1i128 << bits) } else { u };
    Some(CVal::Int(out, ty))
}

fn truth(v: &CVal) -> Option<bool> {
    match v {
        CVal::Int(i, _) => Some(*i != 0),
        CVal::Float(f, _) => Some(*f != 0.0),
        CVal::Str(_) => None,
    }
}

/// The value of `e` with its parameters bound to `args`. `None` for what C
/// leaves undefined in a constant (dividing by zero, an out-of-range shift).
pub fn eval(e: &CExpr, args: &[CVal]) -> Option<CVal> {
    let types: Vec<CType> = args.iter().map(val_type).collect();
    let ty = type_of(e, &types).ok()?;
    Some(match e {
        CExpr::Param(i) => args.get(*i)?.clone(),
        CExpr::Lit(v) => v.clone(),
        // The cast's own type, as written: `(Uint32)0x20` is a `uint32_t`.
        CExpr::Cast(t, inner) => match convert(&eval(inner, args)?, t)? {
            CVal::Int(v, _) => CVal::Int(v, t.clone()),
            CVal::Float(f, _) => CVal::Float(f, t.clone()),
            other => other,
        },
        CExpr::Unary(UnOp::Not, inner) => CVal::Int(!truth(&eval(inner, args)?)? as i128, CType::Int),
        CExpr::Unary(op, inner) => {
            let v = convert(&eval(inner, args)?, &ty)?;
            match (op, v) {
                (UnOp::Plus, v) => v,
                (UnOp::Neg, CVal::Int(i, t)) => convert(&CVal::Int(-i, t.clone()), &t)?,
                (UnOp::Neg, CVal::Float(f, t)) => CVal::Float(-f, t),
                (UnOp::BitNot, CVal::Int(i, t)) => convert(&CVal::Int(!i, t.clone()), &t)?,
                _ => return None,
            }
        }
        CExpr::Binary(BinOp::OrOr, a, b) => {
            CVal::Int((truth(&eval(a, args)?)? || truth(&eval(b, args)?)?) as i128, CType::Int)
        }
        CExpr::Binary(BinOp::AndAnd, a, b) => {
            CVal::Int((truth(&eval(a, args)?)? && truth(&eval(b, args)?)?) as i128, CType::Int)
        }
        CExpr::Binary(op @ (BinOp::Shl | BinOp::Shr), a, b) => {
            let CVal::Int(x, _) = convert(&eval(a, args)?, &ty)? else { return None };
            let CVal::Int(n, _) = eval(b, args)? else { return None };
            let (bits, _) = int_shape(&ty)?;
            if !(0..bits as i128).contains(&n) {
                return None;
            }
            let r = if *op == BinOp::Shl { x << n } else { x >> n };
            convert(&CVal::Int(r, ty.clone()), &ty)?
        }
        CExpr::Binary(op, a, b) => {
            let (va, vb) = (eval(a, args)?, eval(b, args)?);
            let common = match op {
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                    usual(&val_type(&va), &val_type(&vb))
                }
                _ => ty.clone(),
            };
            let (ca, cb) = (convert(&va, &common)?, convert(&vb, &common)?);
            match (ca, cb) {
                (CVal::Int(x, _), CVal::Int(y, _)) => {
                    let cmp = |c: bool| CVal::Int(c as i128, CType::Int);
                    let r = match op {
                        BinOp::Eq => return Some(cmp(x == y)),
                        BinOp::Ne => return Some(cmp(x != y)),
                        BinOp::Lt => return Some(cmp(x < y)),
                        BinOp::Gt => return Some(cmp(x > y)),
                        BinOp::Le => return Some(cmp(x <= y)),
                        BinOp::Ge => return Some(cmp(x >= y)),
                        BinOp::Add => x + y,
                        BinOp::Sub => x - y,
                        BinOp::Mul => x.checked_mul(y)?,
                        BinOp::Div if y == 0 => return None,
                        BinOp::Rem if y == 0 => return None,
                        BinOp::Div => x / y,
                        BinOp::Rem => x % y,
                        BinOp::BitAnd => x & y,
                        BinOp::BitOr => x | y,
                        BinOp::BitXor => x ^ y,
                        _ => return None,
                    };
                    convert(&CVal::Int(r, ty.clone()), &ty)?
                }
                (CVal::Float(x, _), CVal::Float(y, _)) => {
                    let cmp = |c: bool| CVal::Int(c as i128, CType::Int);
                    let r = match op {
                        BinOp::Eq => return Some(cmp(x == y)),
                        BinOp::Ne => return Some(cmp(x != y)),
                        BinOp::Lt => return Some(cmp(x < y)),
                        BinOp::Gt => return Some(cmp(x > y)),
                        BinOp::Le => return Some(cmp(x <= y)),
                        BinOp::Ge => return Some(cmp(x >= y)),
                        BinOp::Add => x + y,
                        BinOp::Sub => x - y,
                        BinOp::Mul => x * y,
                        BinOp::Div => x / y,
                        _ => return None,
                    };
                    convert(&CVal::Float(r, ty.clone()), &ty)?
                }
                _ => return None,
            }
        }
        CExpr::Ternary(c, a, b) => {
            let chosen = if truth(&eval(c, args)?)? { eval(a, args)? } else { eval(b, args)? };
            convert(&chosen, &ty)?
        }
        // Memory and calls are C's to compute.
        CExpr::Member { .. } | CExpr::Call { .. } => return None,
    })
}

pub fn val_type(v: &CVal) -> CType {
    match v {
        CVal::Int(_, t) | CVal::Float(_, t) => t.clone(),
        CVal::Str(_) => CType::Pointer(Box::new(CType::Char)),
    }
}

// ---------------------------------------------------------------------------
// Writing C back out

/// The C spelling of an arithmetic type, without needing any header.
pub fn spell(ty: &CType) -> &'static str {
    match canonical(ty) {
        CType::Bool => "_Bool",
        CType::Char => "char",
        CType::SignedChar => "signed char",
        CType::UnsignedChar => "unsigned char",
        CType::Short => "short",
        CType::UnsignedShort => "unsigned short",
        CType::Int => "int",
        CType::UnsignedInt => "unsigned int",
        CType::Long => "long",
        CType::UnsignedLong => "unsigned long",
        CType::LongLong => "long long",
        CType::UnsignedLongLong => "unsigned long long",
        CType::Float => "float",
        _ => "double",
    }
}

/// A type as a message names it: `int`, `struct point *`, `void *`.
fn describe_type(ty: &CType) -> String {
    match canonical(ty) {
        CType::Pointer(inner) => format!("{} *", describe_type(&inner)),
        CType::Void => "void".to_string(),
        CType::Named(n) => n.rsplit('.').next().unwrap_or(&n).to_string(),
        other if int_shape(&other).is_some() || is_float(&other) => spell(&other).to_string(),
        other => format!("{:?}", other),
    }
}

fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::OrOr => "||",
        BinOp::AndAnd => "&&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::BitAnd => "&",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
    }
}

/// `e` as C, fully parenthesized, every value cast to its type so C reads it
/// as the type this module gave it, and parameter `i` spelled `names[i]`.
pub fn to_c(e: &CExpr, names: &[String]) -> String {
    match e {
        CExpr::Param(i) => names.get(*i).cloned().unwrap_or_default(),
        // An `int` is what a bare literal already is.
        CExpr::Lit(CVal::Int(v, CType::Int)) if *v >= 0 => v.to_string(),
        CExpr::Lit(CVal::Int(v, t)) => format!("(({}){})", spell(t), v),
        CExpr::Lit(CVal::Float(f, t)) => format!("(({}){:?})", spell(t), f),
        CExpr::Lit(CVal::Str(s)) => format!("{:?}", s),
        CExpr::Cast(t, inner) => format!("(({})({}))", spell(t), to_c(inner, names)),
        CExpr::Unary(op, inner) => {
            let o = match op {
                UnOp::Neg => "-",
                UnOp::Plus => "+",
                UnOp::BitNot => "~",
                UnOp::Not => "!",
            };
            format!("({}{})", o, to_c(inner, names))
        }
        CExpr::Binary(op, a, b) => format!("({} {} {})", to_c(a, names), op_text(*op), to_c(b, names)),
        CExpr::Ternary(c, a, b) => format!("({} ? {} : {})", to_c(c, names), to_c(a, names), to_c(b, names)),
        CExpr::Member { base, field, arrow } => {
            format!("({}{}{})", to_c(base, names), if *arrow { "->" } else { "." }, field)
        }
        CExpr::Call { func, args } => {
            let a: Vec<String> = args.iter().map(|x| to_c(x, names)).collect();
            format!("{}({})", func, a.join(", "))
        }
    }
}

// ---------------------------------------------------------------------------
// Function-like macros called from Rask

/// One call of a function-like macro, as the checker typed it: the body the
/// translation wrote (`@c_macro`), its parameter names, and the C types the
/// arguments and the result have at this call (struct.c-interop/PP4).
#[derive(Debug, Clone, PartialEq)]
pub struct MacroCall {
    /// The macro's name in C.
    pub name: String,
    /// Its name in the program, unique across headers: what the C function
    /// a run-time call goes to is named after.
    pub symbol: String,
    pub body: String,
    pub params: Vec<String>,
    pub args: Vec<CType>,
    pub result: CType,
    /// The header a macro that reads memory or calls into C is compiled
    /// with; `None` for arithmetic, which needs none.
    pub header: Option<String>,
    /// Each argument's type as C spells it, then the result's: what the C
    /// function a run-time call goes to declares.
    pub arg_c: Vec<String>,
    pub result_c: String,
}

/// No names: a macro body the translation wrote stands on its own.
struct NoNames;

impl Names for NoNames {
    fn value(&self, _: &str) -> Option<CVal> {
        None
    }
    fn arith_type(&self, _: &str) -> Option<CType> {
        None
    }
    fn function_macro(&self, _: &str) -> Option<(Vec<String>, Vec<CTokenKind>)> {
        None
    }
}

/// A `@c_macro` body, parsed against its parameter names.
pub fn parse_body(body: &str, params: &[String]) -> Result<CExpr, String> {
    let toks: Vec<CTokenKind> = crate::CLexer::new(body)
        .tokenize()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| t.kind)
        .filter(|k| !matches!(k, CTokenKind::Eof | CTokenKind::Newline))
        .collect();
    parse(&toks, params, &NoNames)
}

/// The Rask scalar a C arithmetic type is: `int` is `i32`, `unsigned long`
/// is `u64`. `None` for anything that isn't a number.
pub fn rask_scalar(ty: &CType) -> Option<&'static str> {
    Some(match canonical(ty) {
        CType::Bool => "bool",
        CType::Char => if rask_ast::primitives::c_char_unsigned() { "u8" } else { "i8" },
        CType::SignedChar => "i8",
        CType::UnsignedChar => "u8",
        CType::Short => "i16",
        CType::UnsignedShort => "u16",
        CType::Int => "i32",
        CType::UnsignedInt => "u32",
        CType::Long | CType::LongLong => "i64",
        CType::UnsignedLong | CType::UnsignedLongLong => "u64",
        CType::Float => "f32",
        CType::Double => "f64",
        _ => return None,
    })
}

/// The C arithmetic type a Rask scalar is.
pub fn from_rask_scalar(name: &str) -> Option<CType> {
    Some(match name {
        "bool" => CType::Bool,
        "i8" => CType::SignedChar,
        "u8" => CType::UnsignedChar,
        "i16" => CType::Short,
        "u16" => CType::UnsignedShort,
        "i32" => CType::Int,
        "u32" => CType::UnsignedInt,
        "i64" | "isize" => CType::Long,
        "u64" | "usize" => CType::UnsignedLong,
        "f32" => CType::Float,
        "f64" => CType::Double,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Parsing

/// The C type of an integer literal, by its value and how it was written.
pub fn literal_type(v: i128, unsigned: bool, hex: bool) -> CType {
    let fits_int = v >= i32::MIN as i128 && v <= i32::MAX as i128;
    let fits_uint = v >= 0 && v <= u32::MAX as i128;
    let fits_long = v >= i64::MIN as i128 && v <= i64::MAX as i128;
    if unsigned {
        if fits_uint { CType::UnsignedInt } else { CType::UnsignedLong }
    } else if fits_int {
        CType::Int
    } else if hex && fits_uint {
        CType::UnsignedInt
    } else if fits_long {
        CType::Long
    } else {
        CType::UnsignedLong
    }
}

/// Parse `tokens` as one expression in which `params` are the macro's
/// parameters. Function-like macros in it are expanded first, the way the
/// preprocessor would.
pub fn parse(tokens: &[CTokenKind], params: &[String], names: &dyn Names) -> Result<CExpr, String> {
    let tokens = expand(tokens, params, names, &[])?;
    if tokens.is_empty() {
        return Err("an empty body".to_string());
    }
    let mut p = Parser { toks: &tokens, pos: 0, params, names };
    let e = p.ternary()?;
    match p.peek() {
        None => Ok(e),
        Some(t) => Err(format!("`{}` where the expression should have ended", describe(t))),
    }
}

fn describe(t: &CTokenKind) -> String {
    use CTokenKind as T;
    match t {
        T::Ident(n) => n.clone(),
        T::Semi => ";".into(),
        T::LBrace => "{".into(),
        T::Arrow => "->".into(),
        T::Dot => ".".into(),
        T::Eq => "=".into(),
        T::Comma => ",".into(),
        T::Sizeof => "sizeof".into(),
        other => format!("{:?}", other),
    }
}

struct Parser<'a> {
    toks: &'a [CTokenKind],
    pos: usize,
    params: &'a [String],
    names: &'a dyn Names,
}

/// Expand every function-like macro call in `tokens`, as the preprocessor
/// does: each argument is expanded before it's substituted (except beside a
/// `##`), and the result is scanned again with the macro itself switched off,
/// which is what stops a recursive one. `params` are never macros: they're
/// the expression's own parameters.
fn expand(tokens: &[CTokenKind], params: &[String], names: &dyn Names, expanding: &[String]) -> Result<Vec<CTokenKind>, String> {
    use CTokenKind as T;
    if expanding.len() > 32 {
        return Err("macros nested more than 32 deep".to_string());
    }
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if let (T::Ident(n), Some(T::LParen)) = (&tokens[i], tokens.get(i + 1)) {
            if !params.contains(n) && !expanding.contains(n) {
                if let Some((mparams, body)) = names.function_macro(n) {
                    let (args, next) = split_args(tokens, i + 1)
                        .ok_or_else(|| format!("a call to `{}` without its `)`", n))?;
                    let expanded = args
                        .iter()
                        .map(|a| expand(a, params, names, expanding))
                        .collect::<Result<Vec<_>, _>>()?;
                    let subst = substitute(&mparams, &body, &args, &expanded)
                        .ok_or_else(|| format!("a call to `{}` that doesn't expand", n))?;
                    let mut inside = expanding.to_vec();
                    inside.push(n.clone());
                    out.extend(expand(&subst, params, names, &inside)?);
                    i = next;
                    continue;
                }
            }
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    Ok(out)
}

/// The arguments of the call whose `(` is at `open`, split at the commas
/// outside brackets, and the index after its `)`.
fn split_args(tokens: &[CTokenKind], open: usize) -> Option<(Vec<Vec<CTokenKind>>, usize)> {
    use CTokenKind as T;
    let mut args = vec![Vec::new()];
    let mut depth = 0;
    let mut i = open + 1;
    loop {
        let tok = tokens.get(i)?.clone();
        i += 1;
        match tok {
            T::RParen if depth == 0 => break,
            T::Comma if depth == 0 => args.push(Vec::new()),
            T::LParen => {
                depth += 1;
                args.last_mut()?.push(tok);
            }
            T::RParen => {
                depth -= 1;
                args.last_mut()?.push(tok);
            }
            other => args.last_mut()?.push(other),
        }
    }
    if args.len() == 1 && args[0].is_empty() {
        args.clear();
    }
    Some((args, i))
}

impl Parser<'_> {
    fn peek(&self) -> Option<&CTokenKind> {
        self.toks.get(self.pos)
    }

    fn peek2(&self) -> Option<&CTokenKind> {
        self.toks.get(self.pos + 1)
    }

    fn eat(&mut self, k: &CTokenKind) -> bool {
        if self.peek() == Some(k) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: &CTokenKind, what: &str) -> Result<(), String> {
        if self.eat(k) { Ok(()) } else { Err(format!("a missing {}", what)) }
    }

    fn ternary(&mut self) -> Result<CExpr, String> {
        let cond = self.binary(0)?;
        if !self.eat(&CTokenKind::Question) {
            return Ok(cond);
        }
        let a = self.ternary()?;
        self.expect(&CTokenKind::Colon, "`:`")?;
        let b = self.ternary()?;
        Ok(CExpr::Ternary(Box::new(cond), Box::new(a), Box::new(b)))
    }

    fn op(&self) -> Option<(u8, BinOp)> {
        use CTokenKind as T;
        let two = |a: &T, b: &T| self.peek() == Some(a) && self.peek2() == Some(b);
        Some(match self.peek()? {
            T::Pipe if two(&T::Pipe, &T::Pipe) => (1, BinOp::OrOr),
            T::Ampersand if two(&T::Ampersand, &T::Ampersand) => (2, BinOp::AndAnd),
            T::Pipe => (3, BinOp::BitOr),
            T::Caret => (4, BinOp::BitXor),
            T::Ampersand => (5, BinOp::BitAnd),
            T::EqEq => (6, BinOp::Eq),
            T::BangEq => (6, BinOp::Ne),
            T::Lt => (7, BinOp::Lt),
            T::Gt => (7, BinOp::Gt),
            T::LtEq => (7, BinOp::Le),
            T::GtEq => (7, BinOp::Ge),
            T::LShift => (8, BinOp::Shl),
            T::RShift => (8, BinOp::Shr),
            T::Plus => (9, BinOp::Add),
            T::Minus => (9, BinOp::Sub),
            T::Star => (10, BinOp::Mul),
            T::Slash => (10, BinOp::Div),
            T::Percent => (10, BinOp::Rem),
            _ => return None,
        })
    }

    fn binary(&mut self, min: u8) -> Result<CExpr, String> {
        let mut lhs = self.unary()?;
        while let Some((prec, op)) = self.op() {
            if prec <= min {
                break;
            }
            self.pos += if matches!(op, BinOp::OrOr | BinOp::AndAnd) { 2 } else { 1 };
            let rhs = self.binary(prec)?;
            lhs = CExpr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<CExpr, String> {
        use CTokenKind as T;
        let un = |op, e| Ok(CExpr::Unary(op, Box::new(e)));
        match self.peek().ok_or("an expression that ends too early")? {
            T::Minus => {
                self.pos += 1;
                let e = self.unary()?;
                un(UnOp::Neg, e)
            }
            T::Plus => {
                self.pos += 1;
                let e = self.unary()?;
                un(UnOp::Plus, e)
            }
            T::Tilde => {
                self.pos += 1;
                let e = self.unary()?;
                un(UnOp::BitNot, e)
            }
            T::Bang => {
                self.pos += 1;
                let e = self.unary()?;
                un(UnOp::Not, e)
            }
            T::Ampersand => Err("`&`, which takes an address".to_string()),
            T::Star => Err("`*`, which reads through a pointer".to_string()),
            T::Sizeof => Err("`sizeof`, which Rask asks with `reflect.size_of`".to_string()),
            T::LParen => {
                let save = self.pos;
                self.pos += 1;
                if let Some(ty) = self.cast_type()? {
                    self.expect(&T::RParen, "`)` after a cast")?;
                    let e = self.unary()?;
                    return Ok(CExpr::Cast(ty, Box::new(e)));
                }
                self.pos = save + 1;
                // `(type)(x)` with `type` a parameter: the macro takes a type,
                // which no call from Rask can pass.
                if let (Some(T::Ident(n)), Some(T::RParen)) = (self.peek().cloned(), self.peek2()) {
                    let next = self.toks.get(self.pos + 2);
                    let operand_follows = matches!(
                        next,
                        Some(T::LParen | T::Ident(_) | T::IntLit(_) | T::UIntLit(_) | T::HexLit(_) | T::LongLit(..) | T::FloatLit(..))
                    );
                    if self.params.contains(&n) && operand_follows {
                        return Err(format!("its parameter `{}` used as a type, and a call passes values", n));
                    }
                }
                let e = self.ternary()?;
                self.expect(&T::RParen, "`)`")?;
                self.postfix(e)
            }
            _ => {
                let e = self.primary()?;
                self.postfix(e)
            }
        }
    }

    /// What may follow an operand in C but not in arithmetic: `(s)->flags`
    /// is as much a field read as `s->flags`.
    fn postfix(&mut self, mut e: CExpr) -> Result<CExpr, String> {
        loop {
            match self.peek() {
                Some(t @ (CTokenKind::Arrow | CTokenKind::Dot)) => {
                    let arrow = *t == CTokenKind::Arrow;
                    self.pos += 1;
                    let Some(CTokenKind::Ident(field)) = self.peek().cloned() else {
                        return Err("a field access without a field name".to_string());
                    };
                    self.pos += 1;
                    e = CExpr::Member { base: Box::new(e), field, arrow };
                }
                Some(CTokenKind::LBracket) => return Err("`[ ]`, which indexes memory".to_string()),
                Some(CTokenKind::Eq) => return Err("`=`, which assigns".to_string()),
                _ => return Ok(e),
            }
        }
    }

    /// `f(a, b)` after a function's name: each argument an expression.
    fn call(&mut self, func: String) -> Result<CExpr, String> {
        use CTokenKind as T;
        self.expect(&T::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.eat(&T::RParen) {
            loop {
                args.push(self.ternary()?);
                if self.eat(&T::RParen) {
                    break;
                }
                self.expect(&T::Comma, "`,` between arguments")?;
            }
        }
        Ok(CExpr::Call { func, args })
    }

    /// An arithmetic type written inside a cast's parentheses, or `None` when
    /// what follows isn't one (an ordinary parenthesized expression).
    fn cast_type(&mut self) -> Result<Option<CType>, String> {
        use CTokenKind as T;
        let start = self.pos;
        let mut unsigned = None;
        let mut longs = 0;
        let mut short = false;
        let mut base = None;
        loop {
            match self.peek() {
                Some(T::Const) | Some(T::Volatile) => {}
                Some(T::Unsigned) => unsigned = Some(true),
                Some(T::Signed) => unsigned = Some(false),
                Some(T::Long) => longs += 1,
                Some(T::Short) => short = true,
                Some(T::Int) => base = Some(CType::Int),
                Some(T::Char) => base = Some(CType::Char),
                Some(T::Bool) => base = Some(CType::Bool),
                Some(T::Float) => base = Some(CType::Float),
                Some(T::Double) => base = Some(CType::Double),
                Some(T::Void) | Some(T::Struct) | Some(T::Union) | Some(T::Enum) => {
                    return Err("a cast to a type that isn't a number".to_string());
                }
                Some(T::Ident(n)) if base.is_none() && longs == 0 && !short && unsigned.is_none() => {
                    if self.params.contains(n) || self.peek2() != Some(&T::RParen) {
                        return Ok(None);
                    }
                    let Some(ty) = self.names.arith_type(n) else {
                        if self.names.value(n).is_some() || self.names.function_macro(n).is_some() {
                            return Ok(None);
                        }
                        return Err(format!("a cast to `{}`, which isn't a number type", n));
                    };
                    self.pos += 1;
                    return Ok(Some(ty));
                }
                _ => break,
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Ok(None);
        }
        if self.peek() == Some(&T::Star) {
            return Err("a cast to a pointer".to_string());
        }
        let u = unsigned == Some(true);
        Ok(Some(match base {
            Some(CType::Char) => match unsigned {
                Some(true) => CType::UnsignedChar,
                Some(false) => CType::SignedChar,
                None => CType::Char,
            },
            Some(CType::Float) => CType::Float,
            Some(CType::Double) => CType::Double,
            Some(CType::Bool) => CType::Bool,
            _ if short => if u { CType::UnsignedShort } else { CType::Short },
            _ if longs >= 2 => if u { CType::UnsignedLongLong } else { CType::LongLong },
            _ if longs == 1 => if u { CType::UnsignedLong } else { CType::Long },
            _ => if u { CType::UnsignedInt } else { CType::Int },
        }))
    }

    fn primary(&mut self) -> Result<CExpr, String> {
        use CTokenKind as T;
        let tok = self.peek().ok_or("an expression that ends too early")?.clone();
        self.pos += 1;
        let lit = |v: CVal| Ok(CExpr::Lit(v));
        match tok {
            T::IntLit(v) => lit(CVal::Int(v as i128, literal_type(v as i128, false, false))),
            T::UIntLit(v) => lit(CVal::Int(v as i128, literal_type(v as i128, true, false))),
            T::HexLit(v) => lit(CVal::Int(v as i128, literal_type(v as i128, false, true))),
            T::LongLit(v, unsigned) => {
                lit(CVal::Int(v as i128, if unsigned { CType::UnsignedLong } else { CType::Long }))
            }
            T::CharLit(c) => lit(CVal::Int(c as i128, CType::Int)),
            T::FloatLit(f, single) => {
                let ty = if single { CType::Float } else { CType::Double };
                lit(convert(&CVal::Float(f, CType::Double), &ty).expect("a float converts"))
            }
            T::StringLit(mut s) => {
                // Adjacent literals concatenate.
                while let Some(T::StringLit(next)) = self.peek() {
                    s.push_str(next);
                    self.pos += 1;
                }
                lit(CVal::Str(s))
            }
            T::Ident(name) => {
                if let Some(i) = self.params.iter().position(|p| *p == name) {
                    return Ok(CExpr::Param(i));
                }
                // Every macro call was expanded before parsing; one still
                // here was switched off inside its own expansion.
                if self.peek() == Some(&T::LParen) {
                    if self.names.function_macro(&name).is_some() {
                        return Err(format!("`{}` inside its own expansion", name));
                    }
                    return self.call(name);
                }
                match self.names.value(&name) {
                    Some(v) => lit(v),
                    None => Err(format!("`{}`, which isn't a constant or a parameter", name)),
                }
            }
            other => Err(format!("`{}`, which isn't part of an arithmetic expression", describe(&other))),
        }
    }
}

/// A function-like macro's body with its arguments in place of its
/// parameters, and `##` pasted. A parameter beside a `##` takes its argument
/// as written, any other the argument already expanded, as C does. A paste
/// that makes anything but a name or a suffixed number is more preprocessor
/// than an expression needs: `None`.
fn substitute(
    params: &[String],
    body: &[CTokenKind],
    raw: &[Vec<CTokenKind>],
    expanded: &[Vec<CTokenKind>],
) -> Option<Vec<CTokenKind>> {
    use CTokenKind as T;
    if params.len() != raw.len() || params.iter().any(|p| p == "...") {
        return None;
    }
    let mut out: Vec<CTokenKind> = Vec::new();
    let mut paste = false;
    for (k, tok) in body.iter().enumerate() {
        if *tok == T::DoubleHash {
            paste = true;
            continue;
        }
        let beside_paste = paste || body.get(k + 1) == Some(&T::DoubleHash);
        let pieces: Vec<CTokenKind> = match tok {
            T::Ident(n) => match params.iter().position(|p| p == n) {
                Some(i) if beside_paste => raw[i].clone(),
                Some(i) => expanded[i].clone(),
                None => vec![tok.clone()],
            },
            T::Hash => return None,
            other => vec![other.clone()],
        };
        if paste {
            paste = false;
            let left = out.pop()?;
            let mut rest = pieces.into_iter();
            let right = rest.next()?;
            out.push(paste_tokens(left, right)?);
            out.extend(rest);
        } else {
            out.extend(pieces);
        }
    }
    Some(out)
}

fn paste_tokens(left: CTokenKind, right: CTokenKind) -> Option<CTokenKind> {
    use CTokenKind as T;
    match (left, right) {
        (T::Ident(a), T::Ident(b)) => Some(T::Ident(a + &b)),
        // `0x20 ## UL`: a number given its suffix.
        (num, T::Ident(suffix)) if suffix.chars().all(|c| matches!(c, 'u' | 'U' | 'l' | 'L')) => {
            let unsigned = suffix.contains(['u', 'U']);
            let long = suffix.contains(['l', 'L']);
            let v = match num {
                T::IntLit(v) => v as u64,
                T::UIntLit(v) | T::HexLit(v) | T::LongLit(v, _) => v,
                _ => return None,
            };
            Some(match (unsigned, long) {
                (u, true) => T::LongLit(v, u),
                (true, false) => T::UIntLit(v),
                (false, false) => T::IntLit(v as i64),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CLexer;

    struct Header;
    impl Names for Header {
        fn value(&self, name: &str) -> Option<CVal> {
            (name == "NS_PER_MS").then(|| CVal::Int(1_000_000, CType::UnsignedLong))
        }
        fn arith_type(&self, name: &str) -> Option<CType> {
            (name == "Uint8").then_some(CType::FixedInt { bits: 8, signed: false })
        }
        fn function_macro(&self, name: &str) -> Option<(Vec<String>, Vec<CTokenKind>)> {
            match name {
                "MASK" => Some((vec!["X".into()], toks("(1u << ((X)-1))"))),
                "CAST" => Some((vec!["type".into(), "e".into()], toks("((type)(e))"))),
                _ => None,
            }
        }
    }

    fn toks(src: &str) -> Vec<CTokenKind> {
        CLexer::new(src).tokenize().unwrap().into_iter().map(|t| t.kind)
            .filter(|k| !matches!(k, CTokenKind::Eof | CTokenKind::Newline)).collect()
    }

    fn macro_(params: &[&str], body: &str) -> Result<CExpr, String> {
        let params: Vec<String> = params.iter().map(|p| p.to_string()).collect();
        parse(&toks(body), &params, &Header)
    }

    #[test]
    fn a_macro_takes_its_type_from_its_arguments() {
        let ms = macro_(&["NS"], "((NS) / NS_PER_MS)").unwrap();
        // A u64 argument divides as u64; an int argument is converted to the
        // constant's unsigned long, as C does.
        assert_eq!(type_of(&ms, &[CType::UnsignedLong]).unwrap(), CType::UnsignedLong);
        assert_eq!(type_of(&ms, &[CType::Int]).unwrap(), CType::UnsignedLong);
        let half = macro_(&["x"], "x * 0.5f").unwrap();
        assert_eq!(type_of(&half, &[CType::Int]).unwrap(), CType::Float);
        assert_eq!(type_of(&half, &[CType::Double]).unwrap(), CType::Double);
        let mask = macro_(&["b"], "MASK(b)").unwrap();
        assert_eq!(type_of(&mask, &[CType::Int]).unwrap(), CType::UnsignedInt);
    }

    #[test]
    fn an_argument_is_expanded_before_it_goes_in() {
        // SDL_FOURCC's shape: the macro again inside its own argument. C
        // expands the argument first, so the inner call isn't "inside its own
        // expansion".
        let e = macro_(&["A"], "CAST(unsigned int, CAST(Uint8, (A)))").unwrap();
        assert_eq!(type_of(&e, &[CType::Int]).unwrap(), CType::UnsignedInt);
        assert_eq!(eval(&e, &[CVal::Int(300, CType::Int)]), Some(CVal::Int(44, CType::UnsignedInt)));
    }

    struct Surface;
    impl super::Header for Surface {
        fn field(&self, record: &CType, field: &str) -> Option<CType> {
            (*record == CType::Named("Surface".into()) && field == "flags").then_some(CType::UnsignedInt)
        }
        fn function(&self, name: &str) -> Option<(Vec<CType>, CType, bool)> {
            (name == "twice").then(|| (vec![CType::Int], CType::Int, false))
        }
    }

    #[test]
    fn a_macro_that_reads_memory_is_typed_from_the_header() {
        let ptr = CType::Pointer(Box::new(CType::Named("Surface".into())));
        let locked = macro_(&["S"], "(((S)->flags & 1) != 0)").unwrap();
        assert!(touches_memory(&locked));
        assert_eq!(type_of_in(&locked, &[ptr.clone()], &Surface).unwrap(), CType::Int);
        // Without the header there's nothing to type it by.
        assert!(type_of(&locked, &[ptr]).unwrap_err().contains("flags"));
        let call = macro_(&["x"], "(twice(x) + 1)").unwrap();
        assert_eq!(type_of_in(&call, &[CType::Int], &Surface).unwrap(), CType::Int);
        let swap = macro_(&["x"], "__builtin_bswap32(x)").unwrap();
        assert_eq!(type_of_in(&swap, &[CType::UnsignedInt], &NoHeader).unwrap(), CType::UnsignedInt);
        assert!(!touches_memory(&macro_(&["x"], "(x + 1)").unwrap()));
    }

    #[test]
    fn the_usual_conversions() {
        assert_eq!(usual(&CType::Int, &CType::UnsignedInt), CType::UnsignedInt);
        assert_eq!(usual(&CType::Long, &CType::UnsignedInt), CType::Long);
        assert_eq!(usual(&CType::UnsignedChar, &CType::Short), CType::Int);
        assert_eq!(usual(&CType::Float, &CType::Long), CType::Float);
        assert_eq!(promote(&CType::FixedInt { bits: 8, signed: false }), CType::Int);
    }

    #[test]
    fn evaluates_with_arguments() {
        let ms = macro_(&["NS"], "((NS) / NS_PER_MS)").unwrap();
        assert_eq!(
            eval(&ms, &[CVal::Int(2_500_000, CType::UnsignedLong)]),
            Some(CVal::Int(2, CType::UnsignedLong))
        );
        // C's wrap, in the type the expression has.
        let wrap = macro_(&["a"], "(Uint8)(a + 1)").unwrap();
        assert_eq!(
            eval(&wrap, &[CVal::Int(255, CType::Int)]),
            Some(CVal::Int(0, CType::FixedInt { bits: 8, signed: false }))
        );
    }

    #[test]
    fn writes_c_that_means_the_same() {
        let ms = macro_(&["NS"], "((NS) / NS_PER_MS)").unwrap();
        assert_eq!(to_c(&ms, &["a0".into()]), "(a0 / ((unsigned long)1000000))");
    }

    #[test]
    fn says_what_isnt_arithmetic() {
        assert!(macro_(&["x"], "x[0]").unwrap_err().contains("[ ]"));
        assert!(macro_(&["p"], "*p").unwrap_err().contains("pointer"));
        assert!(macro_(&["x"], "do { x; } while (0)").unwrap_err().contains("do"));

        assert!(macro_(&["type", "e"], "((type)(e))").unwrap_err().contains("as a type"));
    }
}
