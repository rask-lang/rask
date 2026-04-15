# Rask — TODO

Open work. Complements [KNOWN_BUGS.md](KNOWN_BUGS.md) (runtime bugs with failing tests) and [ROADMAP.md](ROADMAP.md) (strategic phases).

---

## Parser

- [ ] **`try`/`else` closure syntax** — `try expr else |e| expr` doesn't parse. Workaround: `match` on the result.
- [ ] **`while`/`if` with `is` pattern** — `while self.current is Token.Newline` doesn't parse. Workaround: `loop { match ... _ => break }`.

## Codegen

- [ ] **SIMD is a stub** — `f32x8`/`i32x4` defined in MirType but passed as pointers. No vector instructions generated. Interpreter simulates element-wise.
- [ ] **Debug info — partial DWARF** — Source locations tracked but DWARF generation is minimal.
- [ ] **Single target — x86-64 only** — Cranelift supports ARM/WASM, compiler doesn't configure them.
- [ ] **Linear resource commitment (L1–L3)** — Ownership checker tracks it, codegen doesn't enforce.

## Build

- [ ] **Build script sandbox** — Cross-platform sandbox for dep build scripts (SB1–SB7).

## C Interop

- [ ] **C header parser backend** — `import c "header.h"` parses and AST plumbing is done. Need actual C declaration parser to translate C decls to Rask AST. Manual `extern "C"` still works.

## Stdlib gaps

Percentages are rough coverage vs spec.

- [ ] **Collections (~90%)** — `AllocError` enum, `vec.with()` block syntax, `SliceDescriptor<T>`.
- [ ] **Time (~85%)** — `SystemTime` type, arithmetic operators on `Duration`.
- [ ] **FS (~90%)** — `OpenOptions` builder, `DirEntry` struct.
- [ ] **Net (~70%)** — `UdpSocket`, `net.resolve()` DNS.
- [ ] **JSON (~70%)** — Typed `encode()`/`decode()` (depends on Encode/Decode traits).
- [ ] **CLI (~60%)** — `cli.Parser` builder, auto-generated `--help`/`--version`, `CliError` enum.
- [ ] **Encoding (~40%)** — Stub file. Auto-derive and field annotations depend on comptime for.
- [ ] **Formatting (~40%)** — Stub file. Full compile-time template checking not implemented.
- [ ] **Bits (~40%)** — Per-integer bit methods (`popcount`, `leading_zeros`) not registered as type methods.
- [ ] **Testing (~85%)** — Doc test extraction (T14–T15).

## Design questions

- [ ] **Task-local storage syntax** — Deferred until M:N scheduler is real and explicit param passing proves inadequate.
- [ ] **String C interop** — `as_c_str()`, `string.from_c()`.
- [ ] **Small string optimization (SSO)** — Hybrid layout: inline ≤15 bytes (no heap, no refcount), refcounted heap for larger. Eliminates atomic overhead for the common case. See `comp.string-refcount-elision` for the heap path.
- [ ] **`pool.remove_with(h, |val| { ... })`** — cascading `@resource` cleanup.
- [ ] **Style guideline** — max 3 context clauses per function.
