<!-- id: comp.incremental -->
<!-- status: proposed -->
<!-- summary: Function-level incremental compilation with per-function object caching and in-place binary patching -->
<!-- depends: compiler/semantic-hash-caching.md, compiler/codegen.md -->

# Incremental Compilation

Rask compiles at function granularity. Each monomorphized function is independently compilable, cached by semantic hash, and patchable in the output binary. Two modes: Phase 1 caches per-function object code and relinks; Phase 2 patches function slots in-place in the ELF, eliminating the relink step entirely.

## Function Granularity

| Rule | Description |
|------|-------------|
| **FG1: Monomorphized function is the unit** | Each `(source_function, [concrete_type_args])` instance is independently compilable to machine code |
| **FG2: MIR self-containment** | `MirFunction` contains all information needed for codegen: params, locals, blocks, types. No cross-function context required. |
| **FG3: Stable function identity** | Each monomorphized function has a `MonoFunctionKey = (source_name, [type_arg_names])` that persists across builds |
| **FG4: SCC invalidation** | Mutually recursive function groups (per `comp.semantic-hash/MK4`) invalidate as a unit |
| **FG5: Signature-driven caller invalidation** | If a function's calling convention changes (params, return type), all callers must recompile. Body-only changes don't affect callers when using indirection (Phase 2). |

<!-- test: skip -->
```rask
// sort<T> with T=i32 produces MonoFunctionKey("sort", ["i32"])
// Changing sort's body invalidates sort$i32
// Changing Point's fields invalidates sort$Point but not sort$i32
func sort<T: Comparable>(items: Vec<T>) {
    for i in 1..items.len() {
        const key = items[i]
        mut j = i
        while j > 0 and items[j - 1] > key {
            items[j] = items[j - 1]
            j -= 1
        }
        items[j] = key
    }
}
```

## Incremental Pipeline

| Rule | Description |
|------|-------------|
| **IC1: Frontend always runs** | Lex, parse, desugar, resolve, typecheck, ownership check run on every rebuild. These are fast (sub-10ms for typical files). |
| **IC2: Hash comparison** | After frontend, compute semantic hashes per `comp.semantic-hash`. Compare against previous build's hash map. |
| **IC3: Changed set** | Functions with changed hashes + their transitive dependents (via Merkle tree per `comp.semantic-hash/MK2`) form the changed set |
| **IC4: Incremental mono** | Only re-monomorphize functions in the changed set. Reuse cached `MonoFunction` instances for unchanged functions. |
| **IC5: Incremental MIR** | Only lower changed functions to MIR. Reuse cached MIR for unchanged functions. |
| **IC6: Incremental codegen** | Only compile changed functions to machine code. Reuse cached object code for unchanged functions. |
| **IC7: Link or patch** | Phase 1: relink all objects. Phase 2: patch changed function slots in binary. |

```
Incremental rebuild flow:

File change detected
  → Full frontend (lex/parse/desugar/resolve/typecheck/ownership)
  → Compute semantic hashes
  → Diff against previous hash map
  → Changed set = changed ∪ transitive dependents
  → For each function:
       if unchanged: load cached object code
       if changed:   mono → MIR → optimize → Cranelift → cache
  → Phase 1: relink all objects with fast linker
  → Phase 2: patch changed slots in ELF
```

## Phase 1: Per-Function Object Caching

| Rule | Description |
|------|-------------|
| **OC1: Individual code blobs** | Each monomorphized function compiles to a standalone code blob (machine code + relocations) via Cranelift `ObjectModule` |
| **OC2: Cache key** | `(source_name, [type_args], semantic_hash, target_triple, build_profile)` per `comp.semantic-hash/CK2` |
| **OC3: Cache location** | `.rk-cache/objects/` — one file per function, named by hash of cache key |
| **OC4: Fast relink** | All function objects + runtime linked by `mold` (or `lld` fallback). Linking is the remaining cost. |
| **OC5: Data section caching** | String literals, vtables, comptime globals cached alongside function objects. Keyed by content hash. |
| **OC6: Link map persistence** | Previous build's link map persisted to `.rk-cache/link-map.bin` for incremental linker input |

### Cache Format

Each cached function blob contains:

```
[4 bytes: magic "RASK"]
[4 bytes: format version]
[8 bytes: semantic hash]
[4 bytes: code size]
[4 bytes: relocation count]
[N bytes: machine code]
[M bytes: relocations (symbol name + offset + type)]
```

### Relink Performance

| Project size | Full link (ld) | Full link (mold) | Expected incremental |
|-------------|----------------|------------------|---------------------|
| 1K functions | ~200ms | ~30ms | ~30ms (relink dominates) |
| 10K functions | ~1.5s | ~100ms | ~100ms |
| 100K functions | ~12s | ~500ms | ~500ms |

Phase 1 is sufficient until relink time exceeds the target cycle time (~100ms). At that point, Phase 2 eliminates relinking.

## Phase 2: In-Place Binary Patching

| Rule | Description |
|------|-------------|
| **BP1: Function slots** | Each function occupies a padded slot in `.text`. Slot capacity ≥ code size, rounded up to allow growth. |
| **BP2: Slot sizing** | Initial capacity = `max(code_size × 2, 64)` bytes, rounded up to 16-byte alignment. Excess filled with `int3` (x86) or `brk` (aarch64). |
| **BP3: Indirection table** | `.rask_got` section contains one 8-byte function pointer per function. All cross-function calls go through this table. |
| **BP4: In-place patch** | If new code fits in existing slot: overwrite slot bytes, no other changes needed |
| **BP5: Slot overflow** | If new code exceeds slot capacity: allocate new slot (2× old capacity) at end of `.text`, update `.rask_got` entry, old slot becomes `jmp` thunk to new location |
| **BP6: Metadata section** | `.rask_meta` section stores slot map and hash map for the patcher to read on next build |
| **BP7: No relink** | Changed functions patched directly in the ELF. No linker invocation. |
| **BP8: Data patching** | New string literals appended to `.rodata`. Changed vtables/comptime globals overwritten in place (fixed-size). |

### Binary Layout

```
ELF Header
.text:
  [slot 0: main()          — capacity 256, used 180]
  [slot 1: sort$i32()      — capacity 512, used 340]
  [slot 2: process()       — capacity 128, used 96]
  ...
  [slot N: (overflow slots appended here)]
.rask_got:
  [0]: &slot_0    // main
  [1]: &slot_1    // sort$i32
  [2]: &slot_2    // process
  ...
.rodata:
  (string literals, vtable data, comptime globals)
.rask_meta:
  (slot map, hash map, GOT map)
```

### Call Indirection

```
// Direct call (current, non-incremental):
call sort$i32           // relative offset, breaks if callee moves

// Indirect call (Phase 2):
mov rax, [rask_got + 8] // load function pointer from GOT
call rax                // indirect call through register
```

| Rule | Description |
|------|-------------|
| **CI1: Dev builds only** | Call indirection used only in dev builds. Release builds use direct calls. |
| **CI2: Overhead** | One extra memory load per call (~1-3ns). Acceptable for dev builds. |
| **CI3: Intra-function calls direct** | Calls within the same function (recursion) use direct relative calls — the function can't move relative to itself |
| **CI4: Runtime calls direct** | Calls to the C runtime use direct PLT/GOT (standard ELF mechanism, not `.rask_got`) |

### Slot Overflow Strategy

When a function outgrows its slot:

```
Old slot (capacity 128, new code needs 200):
  [jmp new_slot]     // 5-byte near jump (x86-64)
  [int3 × 123]       // padding

New slot (capacity 256, appended at end of .text):
  [200 bytes of new code]
  [int3 × 56]        // padding

.rask_got entry updated to point to new slot
```

The old slot's `jmp` thunk handles the (unlikely) case of stale function pointers that haven't gone through the GOT. In practice, all calls go through `.rask_got`, so the thunk is a safety net.

## Metadata Section Format

The `.rask_meta` section is the patcher's state. It persists across builds inside the binary itself.

| Rule | Description |
|------|-------------|
| **MD1: Slot map** | `function_name → (slot_offset, slot_capacity, code_size)` |
| **MD2: Hash map** | `MonoFunctionKey → semantic_hash` |
| **MD3: GOT map** | `function_name → got_index` |
| **MD4: Data map** | `data_name → (section, offset, size)` for strings, vtables, comptime globals |
| **MD5: Version** | Compiler version + target triple. Mismatch triggers full rebuild. |

## Data Section Handling

| Rule | Description |
|------|-------------|
| **DT1: String accumulation** | New string literals appended to `.rodata`. Never removed within a session. |
| **DT2: Vtable fixed size** | Vtables have known size from trait definition (8 bytes per method + 24 bytes header). Overwritten in place when method implementations change. |
| **DT3: Comptime overwrite** | Comptime globals have fixed allocated size. Overwritten in place on recompute. Size increase triggers full rebuild. |
| **DT4: Layout change cascade** | Struct/enum layout size change invalidates all functions using that type (per `comp.semantic-hash/IV4`) AND all vtables for that type |

## Fallback

| Rule | Description |
|------|-------------|
| **FB1: Full rebuild trigger** | Any of: layout size change affecting >50% of functions, signature change cascading beyond threshold, metadata corruption, compiler version mismatch |
| **FB2: Correctness over speed** | If the patcher detects inconsistency, discard and full-rebuild. Never produce a corrupt binary. |
| **FB3: Phase fallback** | Phase 2 failure (e.g., slot overflow beyond capacity) falls back to Phase 1 (full relink). Phase 1 failure falls back to full rebuild. |
| **FB4: Cache corruption** | Detected via checksum in cache header. Corrupt entries discarded, functions recompiled. |

## Compiler Daemon

| Rule | Description |
|------|-------------|
| **DM1: Long-lived process** | `rask build --watch` keeps compiler state in memory across rebuilds |
| **DM2: State contents** | Previous semantic hash map, previous `MonoProgram`, slot map (Phase 2), link map (Phase 1) |
| **DM3: File watcher** | Reuses existing watch infrastructure. On change, triggers incremental pipeline. |
| **DM4: First build full** | Initial build is full compilation. Cache populated. All subsequent builds incremental. |
| **DM5: Non-daemon mode** | `rask build` without `--watch` reads cache from disk (`.rk-cache/`), does one incremental build, writes cache back. No daemon required. |

## Edge Cases

| Case | Handling | Rule |
|------|---------|------|
| New function added | Full mono from `main()` detects new reachable function. New slot allocated (Phase 2) or new .o added (Phase 1). | IC4 |
| Function removed | No longer reachable from `main()`. Slot becomes dead (Phase 2, reclaimed on full rebuild). Cache entry expires. | FG3 |
| Signature change | Merkle propagation invalidates all callers. Callers recompile with new call signature. GOT entries unchanged. | FG5 |
| Struct field added | Layout change. All functions using that struct invalidated. Vtables for that type rebuilt. | DT4 |
| New string literal | Appended to `.rodata` (Phase 2). Added to object cache (Phase 1). | DT1 |
| Generic instantiation set changes | New `(fn, [types])`: fresh compile + new slot. Removed: dead slot. | FG3 |
| Closure body changes | Closure function is a regular monomorphized function. Recompiled normally. | FG1 |
| Comptime expression changes | Comptime re-evaluated. Result bytes overwritten in data section. Functions using result invalidated via `comp.semantic-hash/CM2`. | DT3 |
| Mutual recursion group change | All SCC members invalidated per `comp.semantic-hash/MK4`. All recompiled together. | FG4 |
| First build (no cache) | Full compilation. Cache populated. Phase 2: full ELF written with slots + metadata. | DM4 |
| Cross-package dependency changes | Per `comp.semantic-hash/CP1-CP4`. Upstream metadata diff triggers selective downstream invalidation. | IC3 |
| Slot overflow chain | Function overflows slot 3 times across builds: each time gets 2× capacity at new location. Old slots become `jmp` thunks. | BP5 |
| Concurrent file saves | Daemon debounces (100ms window). Single rebuild after debounce. | DM3 |
| Build error in one function | Function skipped. Previous binary remains valid (that function's old code still in slot). Error reported. | FB2 |

## Error Messages

```
INFO [comp.incremental/IC6]: incremental rebuild — 3 of 847 functions recompiled
   |
   changed: process(), handle_request(), format_response()
   cached:  844 functions (cache hit rate: 99.6%)
   |

total: 12ms (frontend: 4ms, codegen: 3ms, link: 5ms)
```

```
WARN [comp.incremental/FB1]: layout change cascade — falling back to full rebuild
   |
   struct Player changed size (24 → 32 bytes)
   |
   affected: 423 of 847 functions (49.9%)

WHY: Struct layout change invalidates all functions using that type. When >50% are affected, full rebuild is faster than incremental.
```

```
INFO [comp.incremental/BP5]: function `sort$i32` outgrew slot (512 → 624 bytes)
   |
   old slot at offset 0x4200 (capacity 512)
   new slot at offset 0xA800 (capacity 1024)
   GOT entry updated
```

---

## Appendix (non-normative)

### Rationale

**FG1 (monomorphized function is the unit):** The semantic hash spec (`comp.semantic-hash`) already operates at function granularity. Monomorphization means one source function produces multiple compiled instances (`sort$i32`, `sort$Point`). Changing `Point`'s fields should invalidate `sort$Point` but not `sort$i32`. Function-level granularity enables this precision. File-level granularity would miss it — changing one function in a 50-function file would recompile all 50.

**BP2 (2× initial capacity):** Zig uses a similar over-allocation strategy. Common edits (adding a log line, changing a branch) grow a function by 10-30%. 2× headroom means most edits fit in-place for several iterations before overflow. The cost is wasted `.text` space in dev builds — acceptable since dev binaries aren't shipped.

**BP3 (indirection table):** I chose a separate `.rask_got` over Zig's approach of patching relative call offsets. Zig patches every call site when a function moves, which requires tracking all callers. A GOT means moving a function only updates one pointer. The cost is one extra memory load per call (~1-3ns), but dev builds already skip optimizations, so this is noise.

**CI1 (dev builds only):** Release builds use direct calls with full optimization. The indirection table doesn't exist in release binaries. `rask build --release` always does a full build through the LLVM backend (per `comp.codegen/P4`). Incremental compilation is a dev-cycle optimization, not a release concern.

**IC1 (frontend always runs):** I considered incremental parsing (only re-parse changed files) but the complexity isn't worth it yet. Rask's frontend is fast — lexing + parsing + desugaring + resolving + typechecking a 10K-line file takes ~5ms. The expensive phase is codegen, especially with LLVM. If frontend becomes a bottleneck at 500K+ LOC, file-level parse caching (per `comp.semantic-hash/CT1`) is the next step. The incremental codegen architecture doesn't depend on incremental parsing — they're independent optimizations.

**FB1 (>50% threshold):** When more than half the functions need recompilation, the overhead of incremental bookkeeping (hash comparison, selective loading, cache writes) exceeds the cost of a clean full build. The 50% threshold is a heuristic. In practice, cascading layout changes that hit >50% are rare — they indicate a core data structure change where a full rebuild is expected.

**DM5 (non-daemon mode):** Not every developer wants a long-running daemon. `rask build` reads the on-disk cache, does one incremental build, and writes the cache back. This is slower than daemon mode (pays disk I/O for cache load/store) but still much faster than a full build. The cache format is the same in both modes.

### How This Compares to Zig

Zig's incremental compilation uses a custom x86-64/aarch64 backend (not LLVM) that generates machine code directly into padded function slots, with relative call offset patching. Rask's Phase 2 is similar but uses a GOT instead of call-site patching, and uses Cranelift instead of a custom backend.

The key Zig insight that applies: the output binary format must be designed for patching from day one. You can't take a standard `ld` output and start patching it — the function boundaries aren't recorded, there's no slot padding, and relocations are resolved. Phase 2 requires Rask's own binary writer that produces a patchable ELF with the `.rask_got` and `.rask_meta` sections.

What Rask does differently: Rask uses Cranelift (not a custom backend) for dev codegen. This means I don't control instruction encoding at the byte level. Cranelift produces relocatable code blobs that are placed into slots. This is a small abstraction layer that Zig avoids, but the cost is negligible and the benefit is not having to write a custom code generator.

### Implementation Roadmap

| Step | What | Prerequisite |
|------|------|-------------|
| 1 | Implement `comp.semantic-hash` — hash computation, Merkle tree, cache keys | None |
| 2 | Add `MonoFunctionKey` to monomorphization output | None |
| 3 | Add MIR serialization (`serde` derives) | None |
| 4 | Per-function Cranelift compilation (one `ObjectModule` per function, or extract code blob from single module) | Step 1 |
| 5 | Object cache: write/read cached code blobs to `.rk-cache/objects/` | Steps 1, 3, 4 |
| 6 | Incremental relink with `mold` — Phase 1 complete | Step 5 |
| 7 | Make `CodeGenerator` generic over `Module` (preparation for Phase 2 and future JIT) | None |
| 8 | Custom binary writer: produce ELF with `.rask_got`, `.rask_meta`, padded slots | Step 7 |
| 9 | Binary patcher: read `.rask_meta`, patch changed slots, update GOT — Phase 2 complete | Steps 5, 8 |
| 10 | Compiler daemon mode (`rask build --watch`) | Step 6 or 9 |

Steps 1-3 can start immediately and should happen early. They're the "design the IR for function-level granularity from day one" changes — small, low-risk, and they prevent a retrofit later.

### IR Design Requirements (Do Now)

These are the structural properties the IR must have for incremental compilation to work without a retrofit:

1. **`MirFunction` must remain self-contained.** It already is. Don't introduce cross-function state into MIR lowering.
2. **`MonoFunction` must carry a stable identity** (`MonoFunctionKey`). Currently it has `name` and `type_args` but no dedicated identity type.
3. **MIR types must be serializable.** Currently they aren't. Add `serde` derives.
4. **Codegen must be refactorable to per-function granularity.** The current `CodeGenerator` pre-imports ALL functions into each function's namespace. This works for incremental because you still need to declare all functions (for call resolution), but only *define* changed ones.
5. **Function names must be stable across builds.** The current mangling scheme (`sort$i32`) is stable. Don't change it.

### Open Issues

1. **Per-function ObjectModule overhead.** Creating one Cranelift `ObjectModule` per function has setup overhead (~0.1ms per module). For 10K functions on a full build, that's 1 second of overhead. Alternative: compile all functions in one `ObjectModule` but extract individual code blobs. Cranelift's `ObjectProduct` gives access to the raw bytes but not per-function boundaries. May need Cranelift changes or a workaround.

2. **Cross-package incremental.** The spec handles this via `comp.semantic-hash/CP1-CP4` metadata. But the binary patcher needs to handle multiple packages' code in one binary. Slot allocation across package boundaries needs design.

3. **Debug info.** Cranelift DWARF support is incomplete (`comp.codegen` open issue #2). Incremental compilation adds another layer: patched functions need updated DWARF entries. Defer until Cranelift DWARF matures.

4. **Thread safety.** The compiler daemon must handle file changes arriving while a build is in progress. Current plan: debounce + serialize builds. No concurrent builds.

5. **Mold availability.** Phase 1 depends on `mold` for fast linking. If `mold` isn't installed, fall back to `lld`, then `ld`. Include `mold` in recommended dev setup.

### See Also

- `comp.semantic-hash` — Change detection infrastructure (Merkle tree, cache keys, invalidation)
- `comp.codegen` — Compilation pipeline, MIR lowering, Cranelift backend
- `comp.gen-coalesce` — MIR optimization pass (runs on changed functions during incremental)
- `struct.build` — Build system, watch mode, package compilation order
