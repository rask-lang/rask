// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! type.binary/G1–G2: a `@binary` struct's `build` and `parse`, written as
//! ordinary Rask methods.
//!
//! The interpreter used to answer both in Rust and native had neither, so a
//! program that built a packet only ran on the interpreter. Generated as
//! source, both backends run the same code.
//!
//! Every field's place on the wire is known here: field order is the layout
//! (B1) and nothing is padded (B2). So each output byte is one expression over
//! the fields it overlaps, and each field is one expression over the bytes it
//! covers. No loop over bits, no cursor.

use rask_ast::binary::{field_spec, Endian, FieldSpec, Held};
use rask_ast::decl::{Decl, DeclKind, FnDecl, ImportDecl, StructDecl};
use std::collections::HashMap;
use std::fmt::Write;

use crate::interface_defaults::Injected;

/// What the generated `parse` calls `std.bits`'s error type through. Nothing a
/// program writes can be called this, so the import can't collide with one of
/// its own.
pub(crate) const BITS_ALIAS: &str = "__bits";

struct Wire<'a> {
    name: &'a str,
    spec: FieldSpec,
    /// Bit offset from the start of the struct.
    at: u32,
}

/// The struct's fields on the wire, or `None` when a field isn't one the
/// checker accepts. It reports that, and there is nothing to generate.
fn wire(s: &StructDecl) -> Option<Vec<Wire<'_>>> {
    let mut at = 0;
    let mut out = Vec::new();
    for field in &s.fields {
        let spec = field_spec(&field.ty).ok()?;
        // A float's bit pattern has no primitive to come from yet; the
        // checker says so at the field.
        if matches!(spec.held, Held::Float(_)) {
            return None;
        }
        out.push(Wire { name: &field.name, spec, at });
        at += spec.bits;
    }
    Some(out)
}

/// Every `@binary` struct's size in bits, by name. `T.SIZE` and
/// `T.SIZE_BITS` become these numbers (G4).
pub(crate) fn sizes(decls: &[Decl]) -> HashMap<String, u32> {
    decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Struct(s) if is_binary(s) => {
                let bits = s.fields.iter().map(|f| field_spec(&f.ty).map_or(0, |spec| spec.bits)).sum();
                Some((s.name.clone(), bits))
            }
            _ => None,
        })
        .collect()
}

fn is_binary(s: &StructDecl) -> bool {
    s.attrs.iter().any(|a| a.is("binary"))
}

/// Give every `@binary` struct its `build` and `parse`, and the program the
/// import `parse` names its error through. The new methods' positions go in
/// `injected`: their NodeIds come from a parse of their own, so the walk has
/// to hand out fresh ones.
pub(crate) fn inject(decls: &mut Vec<Decl>, injected: &mut Injected) {
    let mut any = false;
    let mut span = None;
    for (index, decl) in decls.iter_mut().enumerate() {
        let DeclKind::Struct(s) = &mut decl.kind else { continue };
        if !is_binary(s) {
            continue;
        }
        let Some(methods) = methods(s) else { continue };
        for mut method in methods {
            method.span = decl.span;
            method.decl_start = decl.span.start;
            injected.entry(index).or_default().insert(s.methods.len());
            s.methods.push(method);
        }
        any = true;
        span.get_or_insert(decl.span);
    }
    if let (true, Some(span)) = (any, span) {
        decls.push(Decl {
            id: rask_ast::NodeId(0),
            kind: DeclKind::Import(ImportDecl {
                path: vec!["bits".to_string()],
                alias: Some(BITS_ALIAS.to_string()),
                is_glob: false,
                is_lazy: false,
            }),
            span,
        });
    }
}

/// `build` and `parse` for `s`, parsed, or `None` when its fields don't say.
fn methods(s: &StructDecl) -> Option<Vec<FnDecl>> {
    let fields = wire(s)?;
    let bits: u32 = fields.iter().map(|f| f.spec.bits).sum();
    let size = bits.div_ceil(8);
    let text = format!(
        "extend {name} {{\n{build}\n{parse}}}\n",
        name = s.name,
        build = build(&fields, size),
        parse = parse(&s.name, &fields, size),
    );
    let lex = rask_lexer::Lexer::new(&text).tokenize();
    if !lex.errors.is_empty() {
        return None;
    }
    let parsed = rask_parser::Parser::new(lex.tokens).parse();
    if !parsed.errors.is_empty() {
        return None;
    }
    let DeclKind::Impl(block) = parsed.decls.into_iter().next()?.kind else { return None };
    Some(block.methods)
}

fn mask(width: u32) -> String {
    format!("{:#x}", (1u64 << width) - 1)
}

/// The bits of field `f` that land in output byte `byte`, as `[from, to)`
/// counted from the field's most significant bit. `None` if it misses.
fn overlap(f: &Wire, byte: u32) -> Option<(u32, u32)> {
    let lo = (byte * 8).max(f.at);
    let hi = (byte * 8 + 8).min(f.at + f.spec.bits);
    (lo < hi).then(|| (lo - f.at, hi - f.at))
}

fn build(fields: &[Wire], size: u32) -> String {
    let mut body = String::new();
    let _ = writeln!(body, "    public func build(self) -> Vec<u8> {{");
    let _ = writeln!(body, "        mut out: Vec<u8> = Vec.with_capacity({size})");
    for (k, f) in fields.iter().enumerate() {
        match f.spec.held {
            Held::Int(name) if name.starts_with('u') => {
                let _ = writeln!(body, "        let __f{k}: u64 = self.{}", f.name);
            }
            Held::Int(_) => {
                let _ = writeln!(body, "        let __f{k}: u64 = self.{}.wrap<u64>()", f.name);
            }
            Held::Float(_) | Held::Bytes(_) => {}
        }
    }
    for byte in 0..size {
        let mut pieces = Vec::new();
        for (k, f) in fields.iter().enumerate() {
            let Some((from, to)) = overlap(f, byte) else { continue };
            let piece = match (f.spec.held, f.spec.endian) {
                (Held::Bytes(_), _) => format!("self.{}[{}].wrap<u64>()", f.name, byte - f.at / 8),
                // Whole bytes, least significant first (F3 keeps it aligned).
                (_, Some(Endian::Little)) => {
                    format!("((__f{k} >> {}) & 0xff)", 8 * (byte - f.at / 8))
                }
                // Most significant bit first (B3).
                _ => {
                    let down = f.spec.bits - to;
                    let up = byte * 8 + 8 - (f.at + to);
                    format!("(((__f{k} >> {down}) & {}) << {up})", mask(to - from))
                }
            };
            pieces.push(piece);
        }
        let value = if pieces.is_empty() { "0".to_string() } else { pieces.join(" | ") };
        let _ = writeln!(body, "        out.push(({value}).wrap<u8>())");
    }
    let _ = writeln!(body, "        return out");
    let _ = writeln!(body, "    }}");
    body
}

fn parse(name: &str, fields: &[Wire], size: u32) -> String {
    let mut body = String::new();
    let _ = writeln!(
        body,
        "    public func parse(data: Vec<u8>) -> ({name}, Vec<u8>) or {BITS_ALIAS}.BinaryParseError {{"
    );
    let _ = writeln!(body, "        if data.len() < {size} {{");
    let _ = writeln!(
        body,
        "            return {BITS_ALIAS}.BinaryParseError.UnexpectedEnd(expected: {size}, actual: data.len())"
    );
    let _ = writeln!(body, "        }}");
    let mut inits = Vec::new();
    for (k, f) in fields.iter().enumerate() {
        let first = f.at / 8;
        let last = (f.at + f.spec.bits).div_ceil(8);
        let value = match (f.spec.held, f.spec.endian) {
            (Held::Bytes(n), _) => {
                let bytes: Vec<String> = (0..n).map(|i| format!("data[{}]", first as usize + i)).collect();
                format!("[{}]", bytes.join(", "))
            }
            (Held::Int(held), endian) => {
                let parts: Vec<String> = (first..last)
                    .map(|byte| {
                        let (from, to) = overlap(f, byte).expect("the field covers this byte");
                        if endian == Some(Endian::Little) {
                            format!("(data[{byte}].wrap<u64>() << {})", 8 * (byte - first))
                        } else {
                            let down = byte * 8 + 8 - (f.at + to);
                            let up = f.spec.bits - to;
                            format!("(((data[{byte}].wrap<u64>() >> {down}) & {}) << {up})", mask(to - from))
                        }
                    })
                    .collect();
                let _ = writeln!(body, "        let __f{k}: u64 = {}", parts.join(" | "));
                if held == "u64" { format!("__f{k}") } else { format!("__f{k}.wrap<{held}>()") }
            }
            (Held::Float(_), _) => unreachable!("floats are refused before this"),
        };
        inits.push(format!("{}: {value}", f.name));
    }
    let _ = writeln!(body, "        mut rest: Vec<u8> = Vec.new()");
    let _ = writeln!(body, "        for __i in {size}..data.len() {{");
    let _ = writeln!(body, "            rest.push(data[__i])");
    let _ = writeln!(body, "        }}");
    let _ = writeln!(body, "        return ({name}({}), rest)", inits.join(", "));
    let _ = writeln!(body, "    }}");
    body
}
