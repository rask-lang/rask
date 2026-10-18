// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What a call site says by itself, before propagation: the size changes a
//! method's declaration states (`@grows`, `@shrinks`, comp.advanced/EF1), and
//! what a name means under the file's imports.
//!
//! Everything else comes from the callee: the stdlib's bodies are inferred
//! like the program's, and a native's declaration says the rest.

use std::collections::HashMap;

use rask_ast::decl::DeclaredEffects;

use crate::Effects;

/// What a method call does to its receiver's size, as the method's
/// declaration says. The checker read it from the method it resolved.
pub fn size_effects(effect: DeclaredEffects) -> Effects {
    Effects { grow: effect.grows, shrink: effect.shrinks, ..Default::default() }
}

/// What a name at a call site stands for, by the program's imports:
/// `import fs.exists` makes `exists` mean `fs.exists`, and `import fs as f`
/// makes `f.exists` mean it too. Anything else means itself, so a program's
/// own `exists` is its own function.
#[derive(Debug, Default)]
pub struct ImportedNames {
    symbols: HashMap<String, String>,
    modules: HashMap<String, String>,
}

impl ImportedNames {
    pub fn from_decls(decls: &[rask_ast::decl::Decl]) -> Self {
        let mut names = Self::default();
        for decl in decls {
            let rask_ast::decl::DeclKind::Import(i) = &decl.kind else { continue };
            if i.is_glob {
                continue;
            }
            match i.path.as_slice() {
                [module] => {
                    if let Some(alias) = &i.alias {
                        names.modules.insert(alias.clone(), module.clone());
                    }
                }
                [.., last] => {
                    let local = i.alias.clone().unwrap_or_else(|| last.clone());
                    names.symbols.insert(local, i.path.join("."));
                }
                [] => {}
            }
        }
        names
    }

    pub fn qualify(&self, name: &str) -> String {
        if let Some((head, rest)) = name.split_once('.') {
            return match self.modules.get(head) {
                Some(module) => format!("{module}.{rest}"),
                None => name.to_string(),
            };
        }
        self.symbols.get(name).cloned().unwrap_or_else(|| name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_effects_come_from_the_declaration() {
        let shrinks = DeclaredEffects { shrinks: true, ..Default::default() };
        assert!(size_effects(shrinks).shrink);
        assert!(!size_effects(shrinks).blocks);
        assert!(size_effects(DeclaredEffects::default()).is_pure());
    }

    #[test]
    fn imports_name_the_callee() {
        use rask_ast::decl::{Decl, DeclKind, ImportDecl};
        let import = |path: &[&str], alias: Option<&str>| Decl {
            id: rask_ast::NodeId(0),
            kind: DeclKind::Import(ImportDecl {
                path: path.iter().map(|p| p.to_string()).collect(),
                alias: alias.map(str::to_string),
                is_glob: false,
                is_lazy: false,
            }),
            span: rask_ast::Span::new(0, 0),
        };
        let names = ImportedNames::from_decls(&[import(&["fs", "exists"], None), import(&["fs"], Some("files"))]);
        assert_eq!(names.qualify("exists"), "fs.exists");
        assert_eq!(names.qualify("files.exists"), "fs.exists");
        assert_eq!(names.qualify("helper"), "helper");
    }
}
