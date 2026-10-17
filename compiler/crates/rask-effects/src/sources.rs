// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What a call to the stdlib does that its callee's body can't show.
//!
//! The stdlib declares it: `@io` on an I/O source, `@waits` on one that blocks
//! until another task or the clock moves (comp.effects/IO1, AS3), `@grows` and
//! `@shrinks` on a method that changes its receiver's size (EF1). This reads
//! the declarations; the effects of everything else come from propagation.

use std::collections::HashMap;

use rask_ast::decl::DeclaredEffects;

use crate::Effects;

/// What the stdlib declares about `callee`: a free function's name, a module
/// function as `module.name`, or a method as `Type.method` with the
/// receiver's declared type, the form the checker resolves a method call to.
/// Anything the stdlib doesn't declare is pure here, and gets its effects
/// from what it calls.
pub fn classify_call(callee: &str) -> Effects {
    from_declared(declared(callee))
}

/// Classify a method call, given what the method's declaration says. The
/// checker read it from the method it resolved; the name only matters for a
/// call it didn't resolve.
pub fn classify_method(callee: &str, effect: DeclaredEffects) -> Effects {
    let mut e = classify_call(callee);
    e.union(from_declared(effect));
    e
}

fn declared(callee: &str) -> DeclaredEffects {
    let reg = rask_stdlib::StubRegistry::load();
    let found = match callee.split_once('.') {
        Some((owner, name)) => reg.lookup_method(owner, name).map(|m| m.effects),
        None => reg.functions().iter().find(|f| f.name == callee).map(|f| f.effects),
    };
    found.unwrap_or_default()
}

/// AS3: waiting is I/O as well as async.
fn from_declared(d: DeclaredEffects) -> Effects {
    Effects {
        io: d.io || d.waits,
        async_: d.waits,
        grow: d.grows,
        shrink: d.shrinks,
        needs_runtime: false,
    }
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
    fn io_sources_classified() {
        let e = classify_call("fs.open");
        assert!(e.io);
        assert!(!e.async_);
        assert!(!e.mutation());
        assert!(classify_call("TcpConnection.read_text").io);

        assert!(classify_call("println").io);
        assert!(classify_call("fs.read_text").io);
        assert!(classify_call("TcpListener.accept").io);
    }

    /// The ones the name list missed: they are marked where they are declared.
    #[test]
    fn io_the_old_list_missed() {
        assert!(classify_call("fs.current_dir").io);
        assert!(classify_call("Command.run").io);
        assert!(classify_call("Process.wait").io);
        assert!(classify_call("http.get").io);
        assert!(classify_call("HttpClient.request").io);
    }

    #[test]
    fn waiting_async_sources_are_io() {
        let e = classify_call("Handle.join");
        assert!(e.io, "AS3: an async source that waits is IO");
        assert!(e.async_);

        let e = classify_call("Sender.send");
        assert!(e.io);
        assert!(e.async_);
    }

    /// A method's effects are what its declaration says; the name only finds
    /// the declaration.
    #[test]
    fn method_classification() {
        let shrinks = DeclaredEffects { shrinks: true, ..Default::default() };
        assert!(classify_method("Vec.remove", shrinks).shrink);
        assert!(!classify_method("Vec.remove", shrinks).io);
        assert!(classify_method("Handle.join", DeclaredEffects::default()).io);
        assert!(!classify_method("Vec.join", DeclaredEffects::default()).io);
        assert!(classify_method("Foo.insert", DeclaredEffects::default()).is_pure());
    }

    #[test]
    fn undeclared_function_is_pure() {
        assert!(classify_call("add").is_pure());
        assert!(classify_call("json.decode").is_pure());
        assert!(classify_call("Vec.len").is_pure());
        assert!(classify_call("Buffer.write").is_pure());
    }

    #[test]
    fn sleep_is_both_io_and_async() {
        let e = classify_call("time.sleep");
        assert!(e.io);
        assert!(e.async_);
        assert!(!e.mutation());
    }
}
