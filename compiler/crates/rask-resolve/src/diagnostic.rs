// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! How name resolution's errors read to the user.

use rask_diagnostics::{Diagnostic, ToDiagnostic};

/// Names the language used to have, and what took their place. Keeping the list
/// here rather than in the resolver means the resolver doesn't have to know
/// anything about renames — it just fails to find the name, as it should.
fn retired_name(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "Cell" => Some((
            "Shared.new(…)` / `Shared<T>",
            "`Cell`, `Shared` and `Mutex` were one concept — one value, several \
             accessors, a scoped view — differing only in what synchronization they \
             took. That is a strategy, not three types, so it moved into \
             `Shared<T, S>`, and `Cell` is the `Local` strategy \
             [analysis.storage-consolidation]",
        )),
        "Owned" => Some((
            "Heap<T>",
            "`Owned` named single ownership, which every Rask value already has, so \
             it distinguished nothing. What differs is the indirection: the value \
             lives on the heap instead of inline [mem.heap]",
        )),
        _ => None,
    }
}

impl ToDiagnostic for crate::ResolveError {
    fn to_diagnostic(&self) -> Diagnostic {
        use crate::ResolveErrorKind::*;

        match &self.kind {
            // A name that used to exist gets told what replaced it. "Not found
            // in this scope" is true and useless for a rename someone is
            // meeting for the first time.
            UndefinedSymbol { name, .. } if retired_name(name).is_some() => {
                let (replacement, why) = retired_name(name).unwrap();
                Diagnostic::error(format!("`{}` is not a type any more", name))
                    .with_code("E0200")
                    .with_primary(self.span, format!("`{}` was removed", name))
                    .with_fix(format!("write `{}`", replacement))
                    .with_why(why)
            }

            // C's and Rust's null spellings. A pointer that may be absent is an
            // optional like any other (mem.unsafe/PT6).
            UndefinedSymbol { name, .. } if matches!(name.as_str(), "null" | "nullptr" | "NULL" | "nil") => {
                Diagnostic::error(format!("there is no `{}`: absence is `none`", name))
                    .with_code("E0200")
                    .with_primary(self.span, "write `none` here")
                    .with_fix("`none`, at a pointer type that may be absent: `let p: *u8? = none`")
                    .with_why("a `*T` is never null. A pointer that may be absent is `*T?`, one word with null for `none`, and it's tested with `p?` or `p is none` like any other optional [mem.unsafe/PT6]")
            }

            UndefinedSymbol { name, similar } => {
                let fix = match similar {
                    Some(s) => format!("did you mean `{}`?", s),
                    None => "check spelling or add an import".to_string(),
                };
                Diagnostic::error(format!("undefined symbol: `{}`", name))
                    .with_code("E0200")
                    .with_primary(self.span, "not found in this scope")
                    .with_fix(fix)
                    .with_why("all symbols must be defined before use — Rask requires explicit imports")
            }

            // structure.modules/IM1: `pkg.Name` follows from `import pkg`. The
            // stdlib's own source is resolved alongside the program and declares
            // each module as a plain type, so the name was in scope whether or
            // not it was imported — native compiled and ran `math.sin(x)` with
            // no import while the interpreter, which binds a module only when it
            // sees the import, died at runtime (#723).
            ModuleNotImported { name, module } => {
                let d = Diagnostic::error(format!("`{}` is not in scope", name))
                    .with_code("E0210")
                    .with_primary(self.span, "needs an import")
                    .with_why(
                        "nothing comes pre-imported. A stdlib name is in scope where the \
                         program asked for it and nowhere else, which is also what leaves \
                         the name free for a program that wants it for something of its \
                         own [structure.modules/IM1]",
                    );
                match module {
                    // `import time.Duration` gives the bare `Duration` this code
                    // is already written against (IM4). Plain `import time`
                    // would mean rewriting the use as `time.Duration`.
                    Some(m) if m != name => d
                        .with_help(format!("add `import {}.{}` at the top of the file", m, name))
                        .with_fix(format!("import {}.{}", m, name)),
                    _ => d
                        .with_help(format!("add `import {}` at the top of the file", name))
                        .with_fix(format!("import {}", name)),
                }
            }

            DuplicateDefinition { name, previous } => {
                Diagnostic::error(format!("duplicate definition: `{}`", name))
                    .with_code("E0201")
                    .with_primary(self.span, "redefined here")
                    .with_secondary(*previous, "previously defined here")
                    .with_help("rename one of the definitions")
                    .with_fix("rename one of the definitions")
                    .with_why("each name can only be defined once in a scope")
            }

            SelfMisplaced { problem } => {
                Diagnostic::error(format!("`self` {}", problem))
                    .with_code("E0219")
                    .with_primary(self.span, "here")
                    .with_help("`self` is the receiver: the first parameter of a method in an `extend` block, a struct body or an interface")
                    .with_why("a call `x.m(a)` passes `x` as the first parameter; a `self` anywhere else has nothing to receive")
            }

            RepeatedBinding { name } => {
                Diagnostic::error(format!("`{}` is bound twice", name))
                    .with_code("E0218")
                    .with_primary(self.span, format!("`{}` appears more than once here", name))
                    .with_fix("rename one, or use `_` for a value you don't need")
                    .with_why("each name in one pattern or parameter list holds one value; with two, the first could never be read")
            }

            DuplicateTest { kind, name, previous } => {
                Diagnostic::error(format!("two {kind}s named \"{name}\""))
                    .with_code("E0217")
                    .with_primary(self.span, "second one here")
                    .with_secondary(*previous, "first one here")
                    .with_help(format!("rename one of them, e.g. `{kind} \"{name} (2)\"`"))
                    .with_why(format!(
                        "a {kind} is picked by its name — by `-f`, in the report, and in \
                         `rask test --sim`'s replay line — so two with one name can't be told apart"
                    ))
            }

            InvalidBreak { label } => {
                let msg = match label {
                    Some(l) => format!("break with label `{}` outside of loop", l),
                    None => "break outside of loop".to_string(),
                };
                Diagnostic::error(msg)
                    .with_code("E0204")
                    .with_primary(self.span, "cannot break here")
                    .with_help("break can only be used inside `loop`, `while`, or `for`")
                    .with_fix("move this `break` inside a `loop`, `while`, or `for` block")
                    .with_why("`break` can only exit loop constructs")
            }

            BreakValueFromComptimeFor => Diagnostic::error("`break` with a value out of a `comptime for`")
                .with_code("E0970")
                .with_primary(self.span, "a `comptime for` has no value to give")
                .with_fix("write `break` alone to stop the comptime loop, and set a variable before it for the value")
                .with_why("the loop is written out once per item before the program runs; `break` skips the items after it, and there is nothing for a value to become (ctrl.comptime/CT70)"),

            UnknownBreakTarget { name, labels } => {
                let mut d = Diagnostic::error(format!(
                    "`{name}` is neither a variable to break with nor a label to break to"
                ))
                .with_code("E0215")
                .with_primary(self.span, "not a variable, and no loop here carries this label");
                d = match labels.split_first() {
                    Some((nearest, _)) => d
                        .with_help(format!(
                            "the enclosing loops are labelled {}",
                            labels
                                .iter()
                                .map(|l| format!("`{l}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .with_fix(format!("break {nearest}")),
                    None => d
                        .with_help("no enclosing loop is labelled — this reads as a break value")
                        .with_fix(format!("declare `{name}`, or drop it and write `break`")),
                };
                d.with_why(
                    "`break x` breaks out with the value `x` unless `x` labels an enclosing loop",
                )
            }

            InvalidContinue { label } => {
                let msg = match label {
                    Some(l) => format!("continue with label `{}` outside of loop", l),
                    None => "continue outside of loop".to_string(),
                };
                Diagnostic::error(msg)
                    .with_code("E0205")
                    .with_primary(self.span, "cannot continue here")
                    .with_help("continue can only be used inside `loop`, `while`, or `for`")
                    .with_fix("move this `continue` inside a `loop`, `while`, or `for` block")
                    .with_why("`continue` can only skip to the next loop iteration")
            }

            SpawnTakesABlock { form, receiver, handed } => {
                let start = match receiver {
                    Some(r) => format!("{r}.spawn"),
                    None => "spawn".to_string(),
                };
                let fix = if form.ends_with("spawn_with") {
                    let v = handed.as_deref().unwrap_or("value");
                    format!(
                        "name `{v}` in the block instead of passing it in — the block runs \
                         once, so it may consume what it captures:\n\n  \
                         {start} {{ … {v} … }}"
                    )
                } else {
                    format!("drop the parentheses and the bars:\n\n  {start} {{ … }}")
                };
                Diagnostic::error(format!("`{}` takes a block, not a closure", form))
                    .with_code("E0915")
                    .with_primary(self.span, format!("write the task as `{start} {{ … }}`"))
                    .with_fix(fix)
                    .with_why(
                        "a task's body is a block that runs once, not a closure value: it may \
                         use up what it captures, and nothing can call it a second time \
                         [conc.async/S1, S6]",
                    )
            }

            InvalidReturn => Diagnostic::error("return outside of function")
                .with_code("E0206")
                .with_primary(self.span, "cannot return here")
                .with_help("return can only be used inside a function body")
                .with_fix("move this `return` inside a function body")
                .with_why("`return` exits the enclosing function — it has no meaning at the top level"),

            UnknownPackage { path } => {
                let path_str = if path.is_empty() {
                    "<empty>".to_string()
                } else {
                    path.join(".")
                };
                let fix = match path.as_slice() {
                    [name] => stdlib_module_for(name),
                    _ => None,
                }
                .unwrap_or_else(|| "check the package name or add it as a dependency".to_string());
                Diagnostic::error(format!("unknown package: `{}`", path_str))
                    .with_code("E0207")
                    .with_primary(self.span, "package not found")
                    .with_fix(fix)
                    .with_why("imported packages must exist in the project or be declared as dependencies")
            }

            ScopedDependencyUse { name, scope } => {
                let keeps_it = if scope == "dev" {
                    "a debug build or `rask test`"
                } else {
                    "the build script"
                };
                let table = format!("[{}-dependencies]", scope);
                Diagnostic::error(format!(
                    "`{}` is in `{}`, and this build doesn't link it",
                    name, table,
                ))
                .with_code("E0216")
                .with_primary(self.span, format!("`{}` is declared under `{}` in rask.toml", name, table))
                .with_help(format!("{} links it; a release build doesn't", keeps_it))
                .with_fix(format!(
                    "move it from `{}` to `[dependencies]` in rask.toml if the shipped program needs it",
                    table,
                ))
                .with_why(
                    "a scoped dependency is declared for one kind of build — if a \
                     release binary could reach it, the scope would be decorative \
                     [struct.build/D4]",
                )
            }

            ShadowsImport { name, module } => {
                // Naming the module the name came from is the difference between
                // a fix the reader can apply and a hunt through their import
                // list.
                let d = Diagnostic::error(match module {
                    Some(m) => format!("`{}` is already in scope from `{}`", name, m),
                    None => format!("`{}` is already in scope from an import", name),
                })
                .with_code("E0208")
                .with_primary(self.span, format!("a second `{}` in the same scope", name))
                .with_why(format!(
                    "one name, one meaning: with both in scope nothing in the source \
                     says which `{}` a use means, and the compiler would be picking for \
                     you [struct.modules/IM8]",
                    name
                ));
                match module {
                    Some(m) => d
                        .with_help(format!(
                            "rename yours, or keep both by aliasing theirs: \
                             `import {}.{} as Std{}`",
                            m, name, name
                        ))
                        .with_fix(format!("import {}.{} as Std{}", m, name, name)),
                    None => d
                        .with_help("rename your declaration, or alias the import")
                        .with_fix(format!("import pkg.{} as Other{}", name, name)),
                }
            }

            ConflictingExtern { name, previous, current, previous_span: _ } => {
                Diagnostic::error(format!(
                    "`{}` is already declared with a different signature", name
                ))
                .with_code("E0213")
                .with_primary(self.span, format!("declared here as `{}`", current))
                // The earlier declaration is usually in the stdlib, and a span
                // from another file renders as a blank line against this one —
                // so name the signature rather than pointing at it.
                .with_note(format!("already declared as `{}`", previous))
                .with_help(format!(
                    "match the existing declaration, or drop this one — `{}` is already in scope",
                    name
                ))
                .with_fix(previous.clone())
                .with_why(
                    "an `extern` name is a single symbol in the linked program, so two \
                     signatures for it can't both be right. The stdlib declares some C \
                     functions itself — std.fs declares `strlen` — and a second declaration \
                     used to replace it silently, which left the stdlib's own calls checked \
                     against the wrong signature",
                )
            }

            ShadowsBuiltin { name } => {
                // Which set the name is in decides what to say about it. A
                // built-in function is worse than a built-in type: the compiler
                // generates code for `println` at each call site, so a program's
                // own would be written and then never called.
                if crate::is_reserved_builtin_fn(name) {
                    Diagnostic::error(format!("`{}` is a built-in function", name))
                        .with_code("E0209")
                        .with_primary(self.span, "this name is already taken")
                        .with_help(format!("rename it — `{}_impl`, or whatever it actually does", name))
                        .with_fix(format!("give it a name of its own, not `{}`", name))
                        .with_why(
                            "the compiler knows this function's signature and generates \
                             code for it at each call site, so it isn't a symbol a \
                             declaration can replace — a program's own would be compiled \
                             and never called [struct.modules/BF2, BF3]",
                        )
                } else {
                    Diagnostic::error(format!("`{}` is a built-in type", name))
                        .with_code("E0209")
                        .with_primary(self.span, "this name is already taken")
                        .with_help("rename the declaration")
                        .with_fix(format!("give it a name of its own, not `{}`", name))
                        .with_why(
                            "the built-in set is fixed — a program can't add to it or \
                             replace a member — so a second type under this name would \
                             have no way to be referred to [struct.modules/BI2, BI3]",
                        )
                }
            }

            ShadowsModule { name, module, imported_at } => {
                let what = if name == module {
                    format!("`{}` is the module imported here", name)
                } else {
                    format!("`{}` is the `{}` module, imported here", name, module)
                };
                Diagnostic::error(format!("`{}` already names an imported module", name))
                    .with_code("E0911")
                    .with_primary(self.span, format!("a second `{}` in the same scope", name))
                    .with_secondary(*imported_at, what)
                    .with_fix(format!(
                        "name this something else, or import the module under another name: \
                         `import {} as {}_mod`",
                        module, module
                    ))
                    .with_why(format!(
                        "one name, one meaning: with both in scope nothing in the source \
                         says whether `{}.x` means the module's member or a field of this \
                         value [struct.modules/IM8]",
                        name
                    ))
            }

            NoSuchStdlibExport { module, symbol, suggestion } => {
                let d = Diagnostic::error(format!(
                    "`{}` has no `{}` to import", module, symbol
                ))
                .with_code("E0212")
                .with_primary(self.span, "not part of this module")
                .with_why(
                    "a selective import names one symbol out of a module; importing a name \
                     the module doesn't have used to be accepted here and only failed later, \
                     at code generation",
                );
                match suggestion {
                    Some(name) => d
                        .with_help(format!("did you mean `import {}.{}`?", module, name))
                        .with_fix(format!("import {}.{}", module, name)),
                    None => d
                        .with_help(format!("import the whole module with `import {}`", module))
                        .with_fix(format!("import {}", module)),
                }
            }
        }
    }
}

/// The stdlib module someone meant by `import <name>`: a name other languages
/// use for it, or a near-miss spelling.
fn stdlib_module_for(name: &str) -> Option<String> {
    match name {
        "env" | "environ" | "dotenv" => {
            Some("environment variables are in `os`: `import os`, then `os.env(\"HOME\")`".to_string())
        }
        "sys" | "process" | "argv" | "args" => {
            Some("the program's arguments are in `os`: `import os`, then `os.args()`".to_string())
        }
        _ => rask_diagnostics::suggestions::did_you_mean(name, rask_stdlib::modules::module_names().iter().copied()),
    }
}

impl ToDiagnostic for crate::UnusedLocal {
    fn to_diagnostic(&self) -> Diagnostic {
        Diagnostic::warning(format!("unused variable `{}`", self.name))
            .with_code("W0901")
            .with_primary(self.span, "nothing reads this")
            .with_fix(format!("use it, or name it `_{}` if that's meant", self.name))
            .with_why("an unread binding is usually a typo or a leftover from an edit [tool.warnings/W3]")
    }
}

impl ToDiagnostic for crate::UnusedImport {
    fn to_diagnostic(&self) -> Diagnostic {
        let (span, fix) = match self.member_span {
            Some(member) => (member, format!("remove `{}` from the braces", self.name)),
            None => (self.span, "remove the import".to_string()),
        };
        Diagnostic::warning(format!("unused import `{}`", self.name))
            .with_code("W0201")
            .with_primary(span, "nothing uses this")
            .with_fix(fix)
            .with_why("an import nothing uses is usually left over from an edit [tool.warnings/W1]")
    }
}
