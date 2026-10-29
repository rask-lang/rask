// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! A stdlib module names nothing from a layer above its own.
//!
//! The stdlib is cut into three layers by what each module needs from the
//! machine (`rask_stdlib::stubs::Layer`, rask-lang/rask#1648): `core` needs
//! nothing, `alloc` a heap, `std` an operating system. The cut is only real if
//! it holds: one `import io` in a module meant for a chip and the no-OS build
//! drags the OS back in. Nothing in the language stops that, because the
//! stdlib's names share one table and `import` is a namespace, not a load.
//!
//! So this walks every stdlib body and signature after resolution and checks
//! each reference against the layer of the file that declares what it names.
//! An edge upward fails the build unless it's in `ALLOWED`, which is the list
//! of modules the issue says still straddle a line, each with the cut that
//! removes it. Add to that list only with a reason; take from it when the cut
//! is made.

use std::collections::{BTreeMap, HashMap};

use rask_ast::decl::{Decl, DeclKind, FnDecl, TypeParam};
use rask_ast::expr::Expr;
use rask_ast::visit::{visit_decl, Visit};
use rask_resolve::SymbolKind;
use rask_stdlib::stubs::{layer_of, stub_sources, Layer, StubRegistry};

/// Upward edges that exist today, `(from, to, why)`. Each is a module that
/// has to be cut in two before its row in the layer table is true.
const ALLOWED: &[(&str, &str, &str)] = &[];

#[test]
fn no_stdlib_module_names_a_layer_above_its_own() {
    let file_names: HashMap<u16, &'static str> = stub_sources().map(|(name, _, id)| (id, name)).collect();
    let layer = |file_id: u16| file_names.get(&file_id).and_then(|n| layer_of(n)).map(|l| (l, file_names[&file_id]));

    let stdlib = StubRegistry::compilable_decls();
    let resolved = rask_resolve::resolve_with_stdlib(&[], &stdlib).unwrap_or_else(|errs| {
        panic!("the stdlib doesn't resolve on its own: {errs:?}");
    });

    // Type names resolve by spelling, not by node, so a name is owned by the
    // one stdlib file that declares a type of that name. A name declared in
    // two files is skipped: nothing here can say which was meant.
    let mut type_owner: HashMap<&str, Option<u16>> = HashMap::new();
    for sym in resolved.symbols.iter() {
        let is_type = matches!(
            sym.kind,
            SymbolKind::Struct { .. } | SymbolKind::Enum { .. } | SymbolKind::Interface { .. } | SymbolKind::TypeAlias { .. }
        );
        if !is_type || !file_names.contains_key(&sym.span.file_id) {
            continue;
        }
        type_owner
            .entry(sym.name.as_str())
            .and_modify(|o| {
                if *o != Some(sym.span.file_id) {
                    *o = None;
                }
            })
            .or_insert(Some(sym.span.file_id));
    }

    // (from file, to file) -> the names that make the edge.
    let mut edges: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
    for decl in &stdlib {
        let from = decl.span.file_id;
        let Some((from_layer, from_name)) = layer(from) else { continue };
        let mut note = |to: u16, what: String| {
            if to == from {
                return;
            }
            if let Some((to_layer, to_name)) = layer(to) {
                if to_layer > from_layer {
                    edges.entry((from_name, to_name)).or_default().push(what);
                }
            }
        };

        // `import io.Reader` names io whether or not anything resolves to it.
        // `import std.reflect` is `reflect` through the `std` namespace.
        if let DeclKind::Import(import) = &decl.kind {
            let module = match import.path.as_slice() {
                [first, second, ..] if first == "std" => second,
                [first, ..] => first,
                [] => continue,
            };
            let module = format!("{module}.rk");
            if let Some((&id, _)) = file_names.iter().find(|(_, n)| **n == module) {
                note(id, format!("import {}", import.path.join(".")));
            }
        }

        // Every expression that resolved to a stdlib symbol.
        let mut refs = Refs { exprs: Vec::new() };
        visit_decl(decl, &mut refs);
        for expr in refs.exprs {
            let Some(sym_id) = resolved.resolutions.get(&expr.id) else { continue };
            let Some(sym) = resolved.symbols.get(*sym_id) else { continue };
            note(sym.span.file_id, sym.name.clone());
        }

        // Every type name in a signature or a field.
        let mut names = Vec::new();
        for_each_type_name(decl, &mut |n| names.push(n.to_string()));
        for n in names {
            if let Some(Some(owner)) = type_owner.get(n.as_str()) {
                note(*owner, n);
            }
        }
    }

    for (from, to, _) in ALLOWED {
        assert!(
            edges.contains_key(&(from, to)),
            "`{from}` no longer names anything in `{to}`; take it out of ALLOWED"
        );
    }

    let mut report = String::new();
    for ((from, to), mut names) in edges {
        if ALLOWED.iter().any(|(f, t, _)| *f == from && *t == to) {
            continue;
        }
        names.sort();
        names.dedup();
        report.push_str(&format!(
            "  {from} ({}) names {to} ({}): {}\n",
            layer_of(from).unwrap().name(),
            layer_of(to).unwrap().name(),
            names.join(", ")
        ));
    }
    assert!(
        report.is_empty(),
        "stdlib modules naming a layer above their own (rask-lang/rask#1648). Move the \
         module up, cut it in two, or add the edge to ALLOWED with the cut that removes it:\n{report}"
    );
}

struct Refs<'a> {
    exprs: Vec<&'a Expr>,
}

impl<'a> Visit<'a> for Refs<'a> {
    fn expr(&mut self, expr: &'a Expr) -> bool {
        self.exprs.push(expr);
        true
    }
}

/// Every type name written in a declaration's signatures, bounds and fields.
fn for_each_type_name(decl: &Decl, f: &mut dyn FnMut(&str)) {
    fn bounds(params: &[TypeParam], f: &mut dyn FnMut(&str)) {
        for p in params {
            for b in &p.bounds {
                b.ty.walk_names(f);
            }
        }
    }
    fn sig(func: &FnDecl, f: &mut dyn FnMut(&str)) {
        bounds(&func.type_params, f);
        for p in &func.params {
            if let Some(ty) = &p.ty {
                ty.walk_names(f);
            }
        }
        if let Some(ret) = &func.ret_ty {
            ret.walk_names(f);
        }
    }
    match &decl.kind {
        DeclKind::Fn(func) => sig(func, f),
        DeclKind::Struct(s) => {
            bounds(&s.type_params, f);
            for field in &s.fields {
                field.ty.walk_names(f);
            }
            s.methods.iter().for_each(|m| sig(m, f));
        }
        DeclKind::Enum(e) => {
            bounds(&e.type_params, f);
            for v in &e.variants {
                for field in &v.fields {
                    field.ty.walk_names(f);
                }
            }
            e.methods.iter().for_each(|m| sig(m, f));
        }
        DeclKind::Interface(i) => i.methods.iter().for_each(|m| sig(m, f)),
        DeclKind::Impl(i) => {
            i.target_ty.walk_names(f);
            if let Some(t) = &i.interface {
                t.walk_names(f);
            }
            i.methods.iter().for_each(|m| sig(m, f));
        }
        _ => {}
    }
}
