---
name: debug-native
description: Debug a natively compiled Rask program that crashes, segfaults, hits SIGILL, prints garbage, is flaky, or disagrees with the interpreter. Also for compiler panics about a name that "belongs to `Vec`", and programs that generate functions nobody calls.
---

# Debugging native Rask

The interpreter (`rask run --interp`) says what the answer should be. Native is
what ships. Start by running both and saving both outputs.

Always capture output unbuffered. `println` is fully buffered to a pipe, so
anything printed before a crash is lost and you'll place the crash too early:

```
stdbuf -o0 -e0 ./binary > log 2>&1
```

## Tools, by symptom

**Segfault.** `rask compile --dump-mir <file>` shows the MIR.
`RASK_RUNTIME_CHECKS=1 ./binary` turns null-deref segfaults into panics with
messages. Compile the C runtime with `-DRASK_DEBUG` for unconditional checks.

**Flaky, or vanishes when reduced.** `RASK_POISON_STACK=1 ./binary` fills the
stack with `0xAA` before `main`, on each worker thread, and on each task's fiber
stack. A slot codegen forgot to write reads as zero on a fresh stack and looks
fine, so those bugs appear only after a program has run a while. Poisoning makes
them fire on the first call. That turned #577 from 40% flaky into 10/10.

**SIGILL.** A Cranelift trap: an `unreachable` was reached, usually a match on an
out-of-range tag. `gdb -batch -ex run -ex 'bt 25' ./binary` gets the frame.

**Compiler panics that a name "belongs to `Vec`" but nothing declares it.** MIR
minted an internal spelling nobody accounted for. `INTERNAL_SPELLINGS` in
`rask-stdlib/src/mir_metadata.rs` says what each one stands for. The panic is
deliberate: the alternative answer, "no declaration, so the caller owns what came
back", frees a string the container still holds.
`RASK_LIST_UNMAPPED_SPELLINGS=1` reports each one and carries on, so one sweep
over the corpus lists them all.

**Output contains functions nobody calls.** `RASK_LIST_WIDENED_CALLS=1` prints
every method call reachability couldn't pin to one receiver. Each one enqueues
every method sharing that bare name, so one line can be twenty functions. A
four-line program used to print 83 of them (#1062).

**Raw `ld` undefined-symbol errors for `rask_*` internals.** Almost always a
stale compiler against a fresh runtime (#1041), not your program. Rebuild:
`cd compiler && cargo build --release -p rask-cli`.

## When a reduction won't reproduce

Stop writing reductions. Copy the real program, instrument it (print each field,
each hop) and let it tell you which one is wrong.
