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
/// known source functions (stdlib IO, async primitives, container structural
/// mutations). Unknown functions return `Effects::default()` — their effects
/// come from transitive propagation.
pub fn classify_call(callee: &str) -> Effects {
    // IO sources (conc.io-context table). The async ones among them are the
    // calls that wait — sleep, a channel op, a join (AS3).
    if is_io_source(callee) {
        return Effects { io: true, async_: is_async_source(callee), grow: false, shrink: false, needs_runtime: false };
    }

    // Container structural mutation sources (EF1: split into Grow/Shrink)
    if is_grow_source(callee) {
        return Effects { io: false, async_: false, grow: true, shrink: false, needs_runtime: false };
    }
    if is_shrink_source(callee) {
        return Effects { io: false, async_: false, grow: false, shrink: true, needs_runtime: false };
    }

    Effects::default()
}

/// Classify a method call: IO and Async by the resolved `Type.method`, and
/// the structural effects by the method's own name, which is what EF1 keys
/// them on (`insert`, `remove`, … on whatever container).
pub fn classify_method(callee: &str, method: &str) -> Effects {
    let mut e = classify_call(callee);
    if is_grow_source(method) {
        e.grow = true;
    }
    if is_shrink_source(method) {
        e.shrink = true;
    }
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

fn is_grow_source(callee: &str) -> bool {
    // Structural growth (EF1: Grow effect).
    matches!(callee, "insert" | "alloc")
}

fn is_shrink_source(callee: &str) -> bool {
    // Structural shrinkage (EF1: Shrink effect).
    matches!(callee, "remove" | "clear" | "drain" | "delete")
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

    /// A container's structural effects come from the method's name, whatever
    /// the receiver; its I/O comes from the receiver's type.
    #[test]
    fn method_classification() {
        assert!(classify_method("Vec.remove", "remove").shrink);
        assert!(!classify_method("Vec.remove", "remove").io);
        assert!(classify_method("Handle.join", "join").io);
        assert!(!classify_method("Vec.join", "join").io);
    }

    #[test]
    fn grow_sources_classified() {
        let e = classify_call("insert");
        assert!(e.grow);
        assert!(!e.shrink);
        assert!(!e.io);

        let e = classify_call("alloc");
        assert!(e.grow);
    }

    #[test]
    fn shrink_sources_classified() {
        let e = classify_call("remove");
        assert!(e.shrink);
        assert!(!e.grow);

        let e = classify_call("delete");
        assert!(e.shrink);

        let e = classify_call("clear");
        assert!(e.shrink);
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
