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
    // IO sources (conc.io-context table)
    if is_io_source(callee) {
        // Some IO sources are also Async (AS3: Async implies IO)
        if is_async_source(callee) {
            return Effects { io: true, async_: true, grow: false, shrink: false };
        }
        return Effects { io: true, async_: false, grow: false, shrink: false };
    }

    // Async-only sources (also get IO via AS3)
    if is_async_source(callee) {
        return Effects { io: true, async_: true, grow: false, shrink: false };
    }

    // Pool structural mutation sources (EF1: split into Grow/Shrink)
    if is_grow_source(callee) {
        return Effects { io: false, async_: false, grow: true, shrink: false };
    }
    if is_shrink_source(callee) {
        return Effects { io: false, async_: false, grow: false, shrink: true };
    }

    Effects::default()
}

fn is_io_source(callee: &str) -> bool {
    matches!(callee,
        // fs module
        "File.open" | "File.read" | "File.write" | "File.close"
        | "open" | "read_file" | "write_file" | "exists"
        | "fs.read_file" | "fs.write_file" | "fs.exists"
        // net module
        | "TcpListener.bind" | "TcpListener.accept"
        | "TcpConnection.read" | "TcpConnection.write"
        | "UdpSocket.send" | "UdpSocket.recv"
        // io module (stdio)
        | "Stdin.read" | "Stdout.write" | "Stderr.write"
        | "print" | "println" | "eprint" | "eprintln"
        // async sources that are also IO (AS3)
        | "sleep" | "timeout"
        | "spawn" | "Channel.send" | "Channel.recv" | "TaskHandle.join"
    )
}

fn is_async_source(callee: &str) -> bool {
    matches!(callee,
        "spawn" | "sleep" | "timeout"
        | "Channel.send" | "Channel.recv"
        | "TaskHandle.join"
    )
}

fn is_grow_source(callee: &str) -> bool {
    // Pool structural growth (EF1: Grow effect).
    matches!(callee,
        "insert" | "pool.insert" | "alloc" | "pool.alloc"
    )
}

fn is_shrink_source(callee: &str) -> bool {
    // Pool structural shrinkage (EF1: Shrink effect).
    matches!(callee,
        "remove" | "pool.remove"
        | "clear" | "pool.clear"
        | "drain" | "pool.drain"
    )
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
        assert!(classify_call("fs.read_file").io);
        assert!(classify_call("TcpListener.accept").io);
    }

    #[test]
    fn async_sources_also_io() {
        let e = classify_call("spawn");
        assert!(e.io, "AS3: Async implies IO");
        assert!(e.async_);

        let e = classify_call("Channel.send");
        assert!(e.io);
        assert!(e.async_);
    }

    #[test]
    fn grow_sources_classified() {
        let e = classify_call("pool.insert");
        assert!(e.grow);
        assert!(!e.shrink);
        assert!(!e.io);

        let e = classify_call("insert");
        assert!(e.grow);
    }

    #[test]
    fn shrink_sources_classified() {
        let e = classify_call("remove");
        assert!(e.shrink);
        assert!(!e.grow);

        let e = classify_call("pool.remove");
        assert!(e.shrink);

        let e = classify_call("pool.clear");
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
