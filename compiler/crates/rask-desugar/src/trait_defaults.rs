// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! TD2: a trait method with a body is inherited by every conformer that doesn't
//! write its own.
//!
//! The copy is injected into the `extend` block as an ordinary method, which is
//! what makes the rest of the compiler need no changes: the checker sees the
//! method really is there (so the conformance check passes and `p.hello()`
//! resolves), and mono, MIR and the interpreter each get a body with `Self`
//! already bound to the concrete type. `self.name()` inside the default resolves
//! against that type, which is the whole point of a copy per conformer rather
//! than one shared function.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use std::collections::{HashMap, HashSet};

/// Injected method positions, keyed by declaration index. The caller gives these
/// bodies fresh NodeIds — a clone carries the trait's own ids, and `node_types`
/// is keyed by id, so two conformers would overwrite each other's inferred
/// types.
pub(crate) type Injected = HashMap<usize, HashSet<usize>>;

fn bare(name: &str) -> String {
    name.split('<').next().unwrap_or(name).trim().to_string()
}

pub(crate) fn inject(decls: &mut [Decl]) -> Injected {
    // Trait name → (super-traits, methods that came with a body).
    let mut traits: HashMap<String, (Vec<String>, Vec<FnDecl>)> = HashMap::new();
    for decl in decls.iter() {
        if let DeclKind::Trait(t) = &decl.kind {
            // A `duck trait` is satisfied by shape, so there's no `extend` block
            // to put a copy in.
            if t.is_duck {
                continue;
            }
            let defaults: Vec<FnDecl> = t
                .methods
                .iter()
                .filter(|m| !m.body.is_empty())
                .cloned()
                .collect();
            traits.insert(bare(&t.name), (t.super_traits.clone(), defaults));
        }
    }
    if traits.values().all(|(_, d)| d.is_empty()) {
        return Injected::new();
    }

    // Every method name each type already has, from anywhere: its own
    // declaration and every `extend` block on it. A default is only inherited
    // where the type says nothing — and "says nothing" has to mean across the
    // whole type, not just the block that names the trait.
    let mut owned: HashMap<String, HashSet<String>> = HashMap::new();
    for decl in decls.iter() {
        let (ty, methods) = match &decl.kind {
            DeclKind::Struct(s) => (bare(&s.name), &s.methods),
            DeclKind::Enum(e) => (bare(&e.name), &e.methods),
            DeclKind::Impl(i) => (bare(&i.target_ty), &i.methods),
            _ => continue,
        };
        let entry = owned.entry(ty).or_default();
        for m in methods {
            entry.insert(m.name.clone());
        }
    }

    let mut injected = Injected::new();
    for (decl_index, decl) in decls.iter_mut().enumerate() {
        let DeclKind::Impl(block) = &mut decl.kind else { continue };
        if block.trait_names.is_empty() {
            continue;
        }
        let target = bare(&block.target_ty);

        // The header's traits and everything above them: a default declared two
        // levels up is still part of what this block promises.
        let mut claimed: Vec<String> = Vec::new();
        let mut queue: Vec<String> = block.trait_names.iter().map(|n| bare(n)).collect();
        while let Some(name) = queue.pop() {
            if claimed.contains(&name) {
                continue;
            }
            if let Some((supers, _)) = traits.get(&name) {
                queue.extend(supers.iter().map(|s| bare(s)));
            }
            claimed.push(name);
        }

        for trait_name in &claimed {
            let Some((_, defaults)) = traits.get(trait_name) else { continue };
            for method in defaults {
                let taken = owned.get(&target).is_some_and(|s| s.contains(&method.name));
                if taken {
                    continue;
                }
                owned.entry(target.clone()).or_default().insert(method.name.clone());
                injected
                    .entry(decl_index)
                    .or_default()
                    .insert(block.methods.len());
                block.methods.push(method.clone());
            }
        }
    }
    injected
}
