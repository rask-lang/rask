// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! std.encoding/E12b: the method beside a type's own `decode` or `encode` that
//! the codecs call.
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
//!
//! `encode` returns the type that is written in its place, and the encoder
//! then writes that the normal way. The native JSON encoder unrolls most
//! shapes itself, but a `Map` it walks at run time has no unrolled code to put
//! the call in, so beside each `encode` this writes
//!
//! ```text
//! func __encode_json(self) -> string { return __json.encode(self.encode()) }
//! ```
//!
//! and the runtime asks for the JSON text of a value of the type.

use rask_ast::decl::{field_attrs, Decl, DeclKind, FnDecl, ImportDecl};

use crate::interface_defaults::Injected;

/// What generated code reaches `json` through. Nothing a program writes can be
/// called this, so the import can't collide with one of its own.
const JSON_ALIAS: &str = "__json";

/// Add the adapter to every `T implements Decode { func decode(…) … }` block,
/// and its mirror to every `T implements Encode { func encode(self) … }`. The
/// new methods' positions go in `injected`: their ids come from a parse of
/// their own, so the walk hands out fresh ones.
pub(crate) fn inject(decls: &mut Vec<Decl>, injected: &mut Injected) {
    let mut writes_json = None;
    for (index, decl) in decls.iter_mut().enumerate() {
        let span = decl.span;
        let DeclKind::Impl(block) = &mut decl.kind else { continue };
        let interface = block.interface.as_ref().and_then(|t| t.last_segment());
        let Some(target) = block.target_ty.last_segment().map(str::to_string) else { continue };
        let mut adapter = match interface {
            Some("Decode") => {
                let Some(decode) = block.methods.iter().find(|m| m.name == "decode") else { continue };
                let [wire] = decode.params.as_slice() else { continue };
                let Some(wire_ty) = wire.ty.clone() else { continue };
                let Some(mut adapter) = decode_adapter(&target) else { continue };
                adapter.params[0].ty = Some(wire_ty);
                adapter
            }
            Some("Encode") => {
                let Some(encode) = block.methods.iter().find(|m| m.name == "encode") else { continue };
                if !encode.params.iter().any(|p| p.name == "self") {
                    continue;
                }
                let Some(adapter) = encode_adapter() else { continue };
                writes_json.get_or_insert(span);
                adapter
            }
            _ => continue,
        };
        adapter.span = span;
        adapter.decl_start = span.start;
        injected.entry(index).or_default().insert(block.methods.len());
        block.methods.push(adapter);
    }
    if let Some(span) = writes_json {
        decls.push(Decl {
            id: rask_ast::NodeId(0),
            kind: DeclKind::Import(ImportDecl {
                path: vec!["json".to_string()],
                alias: Some(JSON_ALIAS.to_string()),
                is_glob: false,
                is_lazy: false,
            }),
            span,
        });
    }
}

fn decode_adapter(target: &str) -> Option<FnDecl> {
    let text = format!(
        "extend Adapter {{\n    func {name}(take wire: Wire) -> Self or string {{\n        \
         let value = {target}.decode(wire) catch e => {{\n            return e.message()\n        }}\n        \
         return value\n    }}\n}}\n",
        name = field_attrs::DECODE_ADAPTER,
        target = target,
    );
    first_method(&text)
}

fn encode_adapter() -> Option<FnDecl> {
    let text = format!(
        "extend Adapter {{\n    func {name}(self) -> string {{\n        \
         return {json}.encode(self.encode())\n    }}\n}}\n",
        name = field_attrs::ENCODE_ADAPTER,
        json = JSON_ALIAS,
    );
    first_method(&text)
}

fn first_method(text: &str) -> Option<FnDecl> {
    let lex = rask_lexer::Lexer::new(text).tokenize();
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
