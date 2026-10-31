// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Integer constant expressions, as C evaluates them in `#define` bodies,
//! enumerator values, array sizes and bitfield widths.
//!
//! Literals, the arithmetic, shift, bitwise, comparison and logical operators,
//! `?:`, parentheses, casts to integer types, and names the caller knows. What
//! it can't reduce to one value is `None`, and the caller skips the
//! declaration rather than guessing (struct.c-interop/PP2).

use crate::{CTokenKind, CType};

/// A value a constant expression produced.
#[derive(Debug, Clone, PartialEq)]
pub enum CValue {
    /// An integer and the C type it has.
    Int(i128, CType),
    Float(f64),
    Str(String),
}

/// What the evaluator asks of its caller.
pub trait Env {
    /// The value of a name: a macro, an enumerator.
    fn value(&self, name: &str) -> Option<CValue>;
    /// The integer type a typedef name stands for, for casts.
    fn int_type(&self, name: &str) -> Option<CType>;
    /// A function-like macro's parameters and body, to expand at a call.
    fn function_macro(&self, _name: &str) -> Option<(Vec<String>, Vec<CTokenKind>)> {
        None
    }
}

/// Evaluate `tokens` as one constant expression.
pub fn eval(tokens: &[CTokenKind], env: &dyn Env) -> Option<CValue> {
    eval_in(tokens, env, &[])
}

/// `tokens`, inside the expansions of the macros in `expanding`. C doesn't
/// expand a macro again inside its own expansion, which also stops a
/// recursive one.
fn eval_in(tokens: &[CTokenKind], env: &dyn Env, expanding: &[String]) -> Option<CValue> {
    if expanding.len() > 32 {
        return None;
    }
    let mut p = Eval { toks: tokens, pos: 0, env, expanding };
    let v = p.ternary()?;
    if p.pos != tokens.len() {
        return None;
    }
    Some(v)
}

/// Evaluate `tokens` and demand an integer.
pub fn eval_int(tokens: &[CTokenKind], env: &dyn Env) -> Option<i128> {
    match eval(tokens, env)? {
        CValue::Int(v, _) => Some(v),
        _ => None,
    }
}

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

/// Size in bits and signedness of an integer type, where C fixes them for the
/// targets Rask builds for (LP64 and LLP64 agree on all but `long`, which is
/// read as 64 bits here; only the wrap of an overflowing cast depends on it).
fn int_shape(ty: &CType) -> Option<(u32, bool)> {
    Some(match ty {
        CType::Char | CType::SignedChar => (8, true),
        CType::UnsignedChar | CType::Bool => (8, false),
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

/// `v` converted to `ty`, wrapping as C does.
fn convert(v: i128, ty: &CType) -> i128 {
    let Some((bits, signed)) = int_shape(ty) else { return v };
    let mask: i128 = if bits >= 128 { -1 } else { (1i128 << bits) - 1 };
    let u = v & mask;
    if signed && bits < 128 && (u >> (bits - 1)) & 1 == 1 {
        u - (1i128 << bits)
    } else {
        u
    }
}

/// The usual arithmetic conversions, reduced to what constants need: the
/// wider operand wins, and unsigned wins a tie.
fn common(a: &CType, b: &CType) -> CType {
    let (ab, asg) = int_shape(a).unwrap_or((32, true));
    let (bb, bsg) = int_shape(b).unwrap_or((32, true));
    let bits = ab.max(bb).max(32);
    let unsigned = (ab == bits && !asg) || (bb == bits && !bsg);
    match (bits, unsigned) {
        (32, false) => CType::Int,
        (32, true) => CType::UnsignedInt,
        (_, false) => CType::Long,
        (_, true) => CType::UnsignedLong,
    }
}

struct Eval<'a> {
    toks: &'a [CTokenKind],
    pos: usize,
    env: &'a dyn Env,
    expanding: &'a [String],
}

impl Eval<'_> {
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

    fn ternary(&mut self) -> Option<CValue> {
        let cond = self.binary(0)?;
        if !self.eat(&CTokenKind::Question) {
            return Some(cond);
        }
        let a = self.ternary()?;
        if !self.eat(&CTokenKind::Colon) {
            return None;
        }
        let b = self.ternary()?;
        let CValue::Int(c, _) = cond else { return None };
        Some(if c != 0 { a } else { b })
    }

    /// The binary operator at the cursor and its precedence, lowest first.
    fn op(&self) -> Option<(u8, Op)> {
        use CTokenKind as T;
        let two = |a: &T, b: &T| self.peek() == Some(a) && self.peek2() == Some(b);
        Some(match self.peek()? {
            T::Pipe if two(&T::Pipe, &T::Pipe) => (1, Op::OrOr),
            T::Ampersand if two(&T::Ampersand, &T::Ampersand) => (2, Op::AndAnd),
            T::Pipe => (3, Op::BitOr),
            T::Caret => (4, Op::BitXor),
            T::Ampersand => (5, Op::BitAnd),
            T::EqEq => (6, Op::Eq),
            T::BangEq => (6, Op::Ne),
            T::Lt => (7, Op::Lt),
            T::Gt => (7, Op::Gt),
            T::LtEq => (7, Op::Le),
            T::GtEq => (7, Op::Ge),
            T::LShift => (8, Op::Shl),
            T::RShift => (8, Op::Shr),
            T::Plus => (9, Op::Add),
            T::Minus => (9, Op::Sub),
            T::Star => (10, Op::Mul),
            T::Slash => (10, Op::Div),
            T::Percent => (10, Op::Rem),
            _ => return None,
        })
    }

    fn binary(&mut self, min: u8) -> Option<CValue> {
        let mut lhs = self.unary()?;
        while let Some((prec, op)) = self.op() {
            if prec <= min {
                break;
            }
            self.pos += if matches!(op, Op::OrOr | Op::AndAnd) { 2 } else { 1 };
            let rhs = self.binary(prec)?;
            lhs = apply(op, lhs, rhs)?;
        }
        Some(lhs)
    }

    fn unary(&mut self) -> Option<CValue> {
        use CTokenKind as T;
        match self.peek()? {
            T::Minus => {
                self.pos += 1;
                match self.unary()? {
                    CValue::Int(v, t) => Some(CValue::Int(convert(-v, &t), t)),
                    CValue::Float(f) => Some(CValue::Float(-f)),
                    CValue::Str(_) => None,
                }
            }
            T::Plus => {
                self.pos += 1;
                self.unary()
            }
            T::Tilde => {
                self.pos += 1;
                match self.unary()? {
                    CValue::Int(v, t) => Some(CValue::Int(convert(!v, &t), t)),
                    _ => None,
                }
            }
            T::Bang => {
                self.pos += 1;
                match self.unary()? {
                    CValue::Int(v, _) => Some(CValue::Int((v == 0) as i128, CType::Int)),
                    _ => None,
                }
            }
            T::LParen => {
                // A cast, if what follows is a type name.
                let save = self.pos;
                self.pos += 1;
                if let Some(ty) = self.cast_type() {
                    if self.eat(&T::RParen) {
                        return match self.unary()? {
                            CValue::Int(v, _) => Some(CValue::Int(convert(v, &ty), ty)),
                            CValue::Float(f) if int_shape(&ty).is_some() => {
                                Some(CValue::Int(convert(f as i128, &ty), ty))
                            }
                            CValue::Float(f) => Some(CValue::Float(f)),
                            CValue::Str(_) => None,
                        };
                    }
                }
                self.pos = save + 1;
                let v = self.ternary()?;
                if !self.eat(&T::RParen) {
                    return None;
                }
                Some(v)
            }
            _ => self.primary(),
        }
    }

    /// An integer or floating type written inside a cast's parentheses.
    fn cast_type(&mut self) -> Option<CType> {
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
                Some(T::Ident(n)) if base.is_none() && longs == 0 && !short && unsigned.is_none() => {
                    let ty = self.env.int_type(n)?;
                    self.pos += 1;
                    return Some(ty);
                }
                _ => break,
            }
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        let u = unsigned == Some(true);
        Some(match base {
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
        })
    }

    fn primary(&mut self) -> Option<CValue> {
        use CTokenKind as T;
        let tok = self.peek()?.clone();
        self.pos += 1;
        match tok {
            T::IntLit(v) => Some(CValue::Int(v as i128, literal_type(v as i128, false, false))),
            T::UIntLit(v) => Some(CValue::Int(v as i128, literal_type(v as i128, true, false))),
            T::HexLit(v) => Some(CValue::Int(v as i128, literal_type(v as i128, false, true))),
            T::LongLit(v, unsigned) => {
                Some(CValue::Int(v as i128, if unsigned { CType::UnsignedLong } else { CType::Long }))
            }
            T::CharLit(c) => Some(CValue::Int(c as i128, CType::Int)),
            T::FloatLit(f) => Some(CValue::Float(f)),
            T::StringLit(mut s) => {
                // Adjacent literals concatenate.
                while let Some(T::StringLit(next)) = self.peek() {
                    s.push_str(next);
                    self.pos += 1;
                }
                Some(CValue::Str(s))
            }
            T::Ident(name) if self.peek() == Some(&T::LParen) && !self.expanding.contains(&name) => {
                match self.env.function_macro(&name) {
                    Some((params, body)) => {
                        let args = self.call_args()?;
                        let expanded = substitute(&params, &body, &args)?;
                        let mut inside = self.expanding.to_vec();
                        inside.push(name);
                        eval_in(&expanded, self.env, &inside)
                    }
                    None => None,
                }
            }
            T::Ident(name) if self.expanding.contains(&name) => None,
            T::Ident(name) => self.env.value(&name),
            _ => None,
        }
    }
}

impl Eval<'_> {
    /// `( a, b )` after a function-like macro's name: each argument's tokens,
    /// split at the commas outside brackets.
    fn call_args(&mut self) -> Option<Vec<Vec<CTokenKind>>> {
        use CTokenKind as T;
        if !self.eat(&T::LParen) {
            return None;
        }
        let mut args = vec![Vec::new()];
        let mut depth = 0;
        loop {
            let tok = self.peek()?.clone();
            self.pos += 1;
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
        Some(args)
    }
}

/// A function-like macro's body with its arguments in place of its
/// parameters, and `##` pasted. A paste that makes anything but a name or a
/// suffixed number is more preprocessor than a constant needs: `None`.
fn substitute(params: &[String], body: &[CTokenKind], args: &[Vec<CTokenKind>]) -> Option<Vec<CTokenKind>> {
    use CTokenKind as T;
    if params.len() != args.len() || params.iter().any(|p| p == "...") {
        return None;
    }
    let mut out: Vec<CTokenKind> = Vec::new();
    let mut paste = false;
    for tok in body {
        if *tok == T::DoubleHash {
            paste = true;
            continue;
        }
        let pieces: Vec<CTokenKind> = match tok {
            T::Ident(n) => match params.iter().position(|p| p == n) {
                Some(i) => args[i].clone(),
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

#[derive(Clone, Copy)]
enum Op {
    OrOr, AndAnd, BitOr, BitXor, BitAnd, Eq, Ne, Lt, Gt, Le, Ge, Shl, Shr, Add, Sub, Mul, Div, Rem,
}

fn apply(op: Op, a: CValue, b: CValue) -> Option<CValue> {
    let (CValue::Int(x, xt), CValue::Int(y, yt)) = (a.clone(), b.clone()) else {
        return float_apply(op, a, b);
    };
    let truth = |c: bool| Some(CValue::Int(c as i128, CType::Int));
    let t = common(&xt, &yt);
    let (cx, cy) = (convert(x, &t), convert(y, &t));
    let v = match op {
        Op::OrOr => return truth(x != 0 || y != 0),
        Op::AndAnd => return truth(x != 0 && y != 0),
        Op::Eq => return truth(cx == cy),
        Op::Ne => return truth(cx != cy),
        Op::Lt => return truth(cx < cy),
        Op::Gt => return truth(cx > cy),
        Op::Le => return truth(cx <= cy),
        Op::Ge => return truth(cx >= cy),
        // A shift has the left operand's (promoted) type.
        Op::Shl | Op::Shr => {
            let lt = common(&xt, &CType::Int);
            if !(0..128).contains(&y) {
                return None;
            }
            let r = if matches!(op, Op::Shl) { x << y } else { x >> y };
            return Some(CValue::Int(convert(r, &lt), lt));
        }
        Op::BitOr => cx | cy,
        Op::BitXor => cx ^ cy,
        Op::BitAnd => cx & cy,
        Op::Add => cx + cy,
        Op::Sub => cx - cy,
        Op::Mul => cx.checked_mul(cy)?,
        Op::Div if cy == 0 => return None,
        Op::Rem if cy == 0 => return None,
        Op::Div => cx / cy,
        Op::Rem => cx % cy,
    };
    Some(CValue::Int(convert(v, &t), t))
}

fn float_apply(op: Op, a: CValue, b: CValue) -> Option<CValue> {
    let f = |v: CValue| match v {
        CValue::Int(i, _) => Some(i as f64),
        CValue::Float(f) => Some(f),
        CValue::Str(_) => None,
    };
    let (x, y) = (f(a)?, f(b)?);
    Some(CValue::Float(match op {
        Op::Add => x + y,
        Op::Sub => x - y,
        Op::Mul => x * y,
        Op::Div => x / y,
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CLexer;

    struct Names;
    impl Env for Names {
        fn value(&self, name: &str) -> Option<CValue> {
            match name {
                "A" => Some(CValue::Int(4, CType::Int)),
                _ => None,
            }
        }
        fn int_type(&self, name: &str) -> Option<CType> {
            (name == "Uint32").then_some(CType::FixedInt { bits: 32, signed: false })
        }
        fn function_macro(&self, name: &str) -> Option<(Vec<String>, Vec<CTokenKind>)> {
            let toks = |src: &str| -> Vec<CTokenKind> {
                CLexer::new(src).tokenize().unwrap().into_iter().map(|t| t.kind)
                    .filter(|k| !matches!(k, CTokenKind::Eof | CTokenKind::Newline)).collect()
            };
            match name {
                "U64" => Some((vec!["c".into()], toks("c ## UL"))),
                "MASK" => Some((vec!["X".into()], toks("(1u << ((X)-1))"))),
                "LOOP" => Some((vec!["x".into()], toks("LOOP(x)"))),
                _ => None,
            }
        }
    }

    fn ev(src: &str) -> Option<CValue> {
        let toks: Vec<CTokenKind> = CLexer::new(src)
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| !matches!(k, CTokenKind::Eof | CTokenKind::Newline))
            .collect();
        eval(&toks, &Names)
    }

    #[test]
    fn shifts_and_flags() {
        assert_eq!(ev("(1 << 3)"), Some(CValue::Int(8, CType::Int)));
        assert_eq!(ev("A | 1"), Some(CValue::Int(5, CType::Int)));
        assert_eq!(ev("1 + 2 * 3"), Some(CValue::Int(7, CType::Int)));
        assert_eq!(ev("(1 + 2) * 3"), Some(CValue::Int(9, CType::Int)));
        assert_eq!(ev("10 - 4 - 3"), Some(CValue::Int(3, CType::Int)));
    }

    #[test]
    fn casts_take_their_type() {
        assert_eq!(
            ev("((Uint32)0x20)"),
            Some(CValue::Int(32, CType::FixedInt { bits: 32, signed: false }))
        );
        assert_eq!(ev("(unsigned int)-1"), Some(CValue::Int(u32::MAX as i128, CType::UnsignedInt)));
        assert_eq!(ev("0x80000000"), Some(CValue::Int(0x8000_0000, CType::UnsignedInt)));
    }

    #[test]
    fn what_it_cant_reduce() {
        assert_eq!(ev("B + 1"), None);
        assert_eq!(ev("sizeof(int)"), None);
        assert_eq!(ev("1 / 0"), None);
        assert_eq!(ev("1 1"), None);
    }

    #[test]
    fn function_like_macros_expand() {
        assert_eq!(ev("U64(0x20)"), Some(CValue::Int(32, CType::UnsignedLong)));
        assert_eq!(ev("~U64(0)"), Some(CValue::Int(u64::MAX as i128, CType::UnsignedLong)));
        assert_eq!(ev("MASK(3)"), Some(CValue::Int(4, CType::UnsignedInt)));
        assert_eq!(ev("MASK(A) | 1"), Some(CValue::Int(9, CType::UnsignedInt)));
        assert_eq!(ev("LOOP(1)"), None);
        assert_eq!(ev("5UL"), Some(CValue::Int(5, CType::UnsignedLong)));
    }

    #[test]
    fn ternary_and_logic() {
        assert_eq!(ev("A > 3 ? 10 : 20"), Some(CValue::Int(10, CType::Int)));
        assert_eq!(ev("1 && 0 || 1"), Some(CValue::Int(1, CType::Int)));
    }
}
