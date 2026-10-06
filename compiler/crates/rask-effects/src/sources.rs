// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Ground-truth effect classification for known source functions.
//!
//! Maps function names to their direct effects based on the tables
//! in `comp.effects` and `conc.io-context` specs.

use crate::Effects;

/// Classify a call target by its known effects.
///
/// Returns non-empty effects only for known source functions (stdlib IO,
/// async primitives, pool structural mutations). Unknown functions return
/// `Effects::default()` — their effects come from transitive propagation.
pub fn classify_call(callee: &str) -> Effects {
    // IO sources (conc.io-context table). The async ones among them are the
    // calls that wait — sleep, a channel op, a join (AS3).
    if is_io_source(callee) {
        return Effects { io: true, async_: is_async_source(callee), grow: false, shrink: false, needs_runtime: false };
    }

    // `spawn` hands the task to the scheduler and returns. It's concurrency,
    // not a wait, so it carries no IO: a loop of spawns doesn't block, and
    // calling it blocking I/O sent CW1/CW2 after the wrong call (#1362).
    if is_async_source(callee) {
        return Effects { io: false, async_: true, grow: false, shrink: false, needs_runtime: false };
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

fn is_io_source(callee: &str) -> bool {
    matches!(callee,
        // fs module
        "File.open" | "File.read" | "File.write" | "File.close"
        | "open" | "read_text" | "write_text" | "exists"
        | "fs.read_text" | "fs.write_text" | "fs.exists"
        // net module
        | "TcpListener.bind" | "TcpListener.accept"
        | "TcpConnection.read" | "TcpConnection.write"
        | "UdpSocket.send" | "UdpSocket.recv"
        // io module (stdio)
        | "Stdin.read" | "Stdout.write" | "Stderr.write"
        | "print" | "println" | "eprint" | "eprintln"
        // async sources that wait (AS3)
        | "sleep" | "timeout"
        | "Channel.send" | "Channel.receive" | "Handle.join"
    )
}

fn is_async_source(callee: &str) -> bool {
    matches!(callee,
        "spawn" | "sleep" | "timeout"
        | "Channel.send" | "Channel.receive"
        | "Handle.join"
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
        let e = classify_call("File.open");
        assert!(e.io);
        assert!(!e.async_);
        assert!(!e.mutation());

        assert!(classify_call("println").io);
        assert!(classify_call("fs.read_text").io);
        assert!(classify_call("TcpListener.accept").io);
    }

    #[test]
    fn waiting_async_sources_are_io() {
        let e = classify_call("Handle.join");
        assert!(e.io, "AS3: an async source that waits is IO");
        assert!(e.async_);

        let e = classify_call("Channel.send");
        assert!(e.io);
        assert!(e.async_);
    }

    /// Starting a task returns at once. Classing it as I/O told every spawn
    /// loop it blocked on each iteration (#1362).
    #[test]
    fn spawn_is_async_without_io() {
        let e = classify_call("spawn");
        assert!(e.async_);
        assert!(!e.io);
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
