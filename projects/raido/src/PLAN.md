# Raido in Rask -- Implementation State

The spec is in `../language/` and `../vm/`. This file says what the Rask
implementation has and what it doesn't yet.

This implementation is a reference. The maintainer is building Raido from
scratch by hand as a test project; the code here is something to compare
against, not the thing to finish.

Any further VM work goes in this order: the spec first, then conformance
tests, then the implementation. The tests are written against the spec, not
against this code, so any implementation (the hand-built one included) can
be checked by running them.

## Files

```
value.rk      16-byte tagged values; optionals nest by a depth in the none tag
heap.rk       GC heap: objects, mark-and-sweep, logical sizes, checkpoints,
              undo log, equality, host roots and generation-checked handles,
              the register stack
serialize.rk  canonical snapshot: objects numbered by a walk from the roots
vm.rk         dispatch loop, frames, calls and closures, transactions, PRNG seed
chunk.rk      Prototype (flat list), StructLayout, Chunk
opcodes.rk    encode/decode/disassemble
asm.rk        hand assembly for tests and the demo
tests.rk      `rask test src` and `rask test --interp src`
lexer.rk      tokenizer
compiler.rk   skeleton: declaration scan and expression compiler are TODO
types.rk      type table for the compiler
main.rk       demo: runs hand-assembled fib
```

## Done (rask-lang/rask#1736)

- Tagged values, 16 bytes.
- The heap: non-moving mark-and-sweep. Roots are module variables, host roots,
  live registers, the undo log and the last result. The memory limit counts
  logical sizes, with capacity computed from length, and is checked at
  checkpoints every limit/8 allocated bytes. `stress` collects at every
  allocation; tests check it changes neither results, snapshot bytes nor the
  instruction where memory runs out.
- Canonical serialization and restore.
- Closures: `CLOSURE`, `GET_CAPTURE`, calls through a closure, and cells
  (`NEW_CELL`, `GET_CELL`, `SET_CELL`) for captured variables that change.
- Transactions: every `vm.call` is a run; writes to objects older than the run,
  module variable writes and the PRNG state roll back on failure.
- Module variables: `GET_MODULE`, `SET_MODULE`, an `init` prototype run at load.
- `IDIV`; `TRY` is gone.

## Not yet

- `apply_undo` borrows its entry and clones what it restores, to dodge a
  native double free (rask-lang/rask#1748). Drop that once it's fixed.

- The compiler. Until it exists, deciding which captured variables need cells, the published API
  module
  and the provable profile have nothing to live in.
- Externs: `CALL_EXTERN`, `GET_FIELD`/`SET_FIELD`, the write buffer,
  deferred and pausing calls, tasks, `SPAWN`, `vm.resume` and
  `vm.spawned`, undo callbacks.
- `CONCAT`, `ENUM_TAG`, `TAIL_CALL`, enums with payloads, string constants,
  the built-in methods (`push`, `sort`, ...), PRNG draws.
- The spec's instruction set is now typed (`ADD_I`/`ADD_F`, `ITOF`, `LT_S`,
  `NEW_ENUM`, `CALL_BUILTIN`, map iteration, `RETURN_VOID`) and verified at
  decode. The VM still has one `ADD` that reads the tag, and no verifier.
