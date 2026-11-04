// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Integer constant expressions, as C evaluates them in `#define` bodies,
//! enumerator values, array sizes and bitfield widths.
//!
//! A constant is an expression with no parameters, so this is `cexpr` asked
//! for a value: the same parser and the same C typing a function-like macro
//! gets. What it can't reduce to one value is `None`, and the caller skips the
//! declaration rather than guessing (struct.c-interop/PP2).

use crate::cexpr::{self, CVal};
use crate::{CTokenKind, CType};

pub use crate::cexpr::literal_type;

/// A value a constant expression produced.
#[derive(Debug, Clone, PartialEq)]
pub enum CValue {
    /// An integer and the C type it has.
    Int(i128, CType),
    /// A floating value and whether C types it `float` (an `f` suffix, a
    /// cast) rather than `double`.
    Float(f64, CType),
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

/// An `Env` as `cexpr` asks for names.
pub(crate) struct AsNames<'a>(pub &'a dyn Env);

impl cexpr::Names for AsNames<'_> {
    fn value(&self, name: &str) -> Option<CVal> {
        Some(match self.0.value(name)? {
            CValue::Int(v, t) => CVal::Int(v, t),
            CValue::Float(f, t) => CVal::Float(f, t),
            CValue::Str(s) => CVal::Str(s),
        })
    }
    fn arith_type(&self, name: &str) -> Option<CType> {
        self.0.int_type(name)
    }
    fn function_macro(&self, name: &str) -> Option<(Vec<String>, Vec<CTokenKind>)> {
        self.0.function_macro(name)
    }
}

/// Evaluate `tokens` as one constant expression.
pub fn eval(tokens: &[CTokenKind], env: &dyn Env) -> Option<CValue> {
    let e = cexpr::parse(tokens, &[], &AsNames(env)).ok()?;
    Some(match cexpr::eval(&e, &[])? {
        CVal::Int(v, t) => CValue::Int(v, t),
        CVal::Float(f, t) => CValue::Float(f, t),
        CVal::Str(s) => CValue::Str(s),
    })
}

/// Evaluate `tokens` and demand an integer.
pub fn eval_int(tokens: &[CTokenKind], env: &dyn Env) -> Option<i128> {
    match eval(tokens, env)? {
        CValue::Int(v, _) => Some(v),
        _ => None,
    }
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
