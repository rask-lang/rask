// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! std.encoding/E12b: the method every decoder calls for a type that writes
//! its own `decode`.
//!
//! `decode` rejects with whatever error type its author chose, which can be a
//! union or anything with a `message()`. A decoder needs only the message and
//! where in the input it happened. So beside each `decode` this writes
//!
//! ```text
//! func __decode_checked(take wire: W) -> Self or string {
//!     let value = T.decode(wire) catch e => { return e.message() }
//!     return value
//! }
//! ```
//!
//! as an ordinary method, so the checker types it, monomorphization reaches
//! it and both backends run it, and the C runtime and the interpreter call one
//! function with a fixed shape instead of each learning how to take a rejection
//! apart. It takes the wire value so that it is dropped here, where drop code
//! for every type exists, whatever `decode` does with its own parameter. The
//! checker doesn't hold a Copy wire's `take` against it (`check_fn`).

use rask_ast::decl::{field_attrs, Decl, DeclKind, FnDecl};

use crate::interface_defaults::Injected;

/// Add the adapter to every `T implements Decode { func decode(…) … }` block.
/// The new method's position goes in `injected`: its ids come from a parse of
/// its own, so the walk hands out fresh ones.
pub(crate) fn inject(decls: &mut [Decl], injected: &mut Injected) {
    for (index, decl) in decls.iter_mut().enumerate() {
        let span = decl.span;
        let DeclKind::Impl(block) = &mut decl.kind else { continue };
        let writes_decode = block.interface.as_ref().and_then(|t| t.last_segment()) == Some("Decode");
        if !writes_decode {
            continue;
        }
        let Some(decode) = block.methods.iter().find(|m| m.name == "decode") else { continue };
        let [wire] = decode.params.as_slice() else { continue };
        let Some(wire_ty) = wire.ty.clone() else { continue };
        let Some(target) = block.target_ty.last_segment().map(str::to_string) else { continue };
        let Some(mut adapter) = adapter(&target) else { continue };
        adapter.params[0].ty = Some(wire_ty);
        adapter.span = span;
        adapter.decl_start = span.start;
        injected.entry(index).or_default().insert(block.methods.len());
        block.methods.push(adapter);
    }
}

fn adapter(target: &str) -> Option<FnDecl> {
    let text = format!(
        "extend Adapter {{\n    func {name}(take wire: Wire) -> Self or string {{\n        \
         let value = {target}.decode(wire) catch e => {{\n            return e.message()\n        }}\n        \
         return value\n    }}\n}}\n",
        name = field_attrs::DECODE_ADAPTER,
        target = target,
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
    block.methods.into_iter().next()
}
