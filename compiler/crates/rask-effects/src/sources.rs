// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Ground-truth effect classification for known source functions.
//!
//! Maps function names to their direct effects based on the tables
//! in `comp.effects` and `conc.io-context` specs.

use crate::Effects;

/// Classify a call target by its known effects.
///
/// `callee` is a free function's name, a module function as `module.name`,
/// or a method as `Type.method` with the receiver's declared type — the form
/// the checker resolves a method call to. Returns non-empty effects only for
/// known source functions (stdlib IO, async primitives). Unknown functions
/// return `Effects::default()` — their effects come from transitive
/// propagation.
pub fn classify_call(callee: &str) -> Effects {
    // IO sources (conc.io-context table). The async ones among them are the
    // calls that wait — sleep, a channel op, a join (AS3).
    if is_io_source(callee) {
        return Effects { io: true, async_: is_async_source(callee), grow: false, shrink: false, needs_runtime: false };
    }
    Effects::default()
}

/// Classify a method call: IO and Async by the resolved `Type.method`, and
/// the structural effects (EF1) by what the method's declaration says
/// (`@grows`, `@shrinks`). They used to come from the bare method name, on
/// any receiver: a program's own `Foo.insert` counted as growing a
/// collection, and `push` and `pop` counted as nothing (#1560).
pub fn classify_method(callee: &str, effect: rask_ast::decl::StructuralEffect) -> Effects {
    let mut e = classify_call(callee);
    e.grow |= effect.grows;
    e.shrink |= effect.shrinks;
    e
}

/// The stdlib's I/O, spelled the way the checker resolves a call to it.
fn is_io_source(callee: &str) -> bool {
    matches!(callee,
        // fs module
        "fs.open" | "fs.create_file" | "fs.read_text" | "fs.read_bytes" | "fs.read_lines"
        | "fs.write_text" | "fs.write_bytes" | "fs.append_text" | "fs.exists"
        | "fs.copy" | "fs.rename" | "fs.remove_file" | "fs.create_dir" | "fs.create_dir_all"
        | "fs.list_dir" | "fs.metadata" | "fs.absolute_path"
        // A free function imported by name (`import fs.read_text`).
        | "open" | "read_text" | "write_text" | "exists"
        // An open file
        | "File.read_text" | "File.read_bytes" | "File.write" | "File.write_bytes"
        | "File.write_text" | "File.write_line" | "File.close"
        // net module
        | "net.tcp_listen" | "net.tcp_connect"
        | "TcpListener.accept" | "TcpListener.close"
        | "TcpConnection.read_text" | "TcpConnection.read_bytes"
        | "TcpConnection.write_text" | "TcpConnection.write_bytes" | "TcpConnection.close"
        // io module (stdio)
        | "io.read_line"
        | "Stdin.read" | "Stdin.read_bytes" | "Stdin.read_text" | "Stdin.read_line"
        | "Stdout.write" | "Stdout.write_bytes" | "Stdout.write_text" | "Stdout.flush"
        | "Stderr.write" | "Stderr.write_bytes" | "Stderr.write_text" | "Stderr.flush"
        | "print" | "println" | "eprint" | "eprintln"
        // async sources that wait (AS3)
        | "sleep" | "time.sleep"
        | "Sender.send" | "Receiver.receive" | "Handle.join" | "Handles.join_all"
    )
}

fn is_async_source(callee: &str) -> bool {
    matches!(callee,
        "sleep" | "time.sleep"
        | "Sender.send" | "Receiver.receive"
        | "Handle.join" | "Handles.join_all"
    )
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

    #[test]
    fn waiting_async_sources_are_io() {
        let e = classify_call("Handle.join");
        assert!(e.io, "AS3: an async source that waits is IO");
        assert!(e.async_);

        let e = classify_call("Sender.send");
        assert!(e.io);
        assert!(e.async_);
    }

    /// A method's structural effects are what its declaration says; its I/O
    /// comes from the receiver's type.
    #[test]
    fn method_classification() {
        use rask_ast::decl::StructuralEffect;
        let shrinks = StructuralEffect { grows: false, shrinks: true };
        assert!(classify_method("Vec.remove", shrinks).shrink);
        assert!(!classify_method("Vec.remove", shrinks).io);
        assert!(classify_method("Handle.join", StructuralEffect::default()).io);
        assert!(!classify_method("Vec.join", StructuralEffect::default()).io);
        assert!(classify_method("Foo.insert", StructuralEffect::default()).is_pure());
    }

    #[test]
    fn unknown_function_is_pure() {
        let e = classify_call("add");
        assert!(e.is_pure());

        let e = classify_call("json.decode");
        assert!(e.is_pure());

        let e = classify_call("Vec.push");
        assert!(e.is_pure());
    }

    #[test]
    fn sleep_is_both_io_and_async() {
        let e = classify_call("sleep");
        assert!(e.io);
        assert!(e.async_);
        assert!(!e.mutation());
    }
}
