# Debugging

**Status:** Design exploration
**Spec ID:** `tool.debug`

Rask's restrictions (single ownership, pools, structured concurrency) make the program state space more tractable than C/Rust. A conventional debugger treats memory as an opaque soup. Rask knows the structure — that's the unfair advantage.

## Tier 1 — DWARF + Existing Tools

### D1. DWARF debug info emission

Emit DWARF sections from Cranelift so GDB/LLDB/codelldb work out of the box.

**What's needed:**
- `gimli::write` builds DWARF sections (`.debug_info`, `.debug_line`, `.debug_abbrev`)
- `cranelift-object`'s `WriteDebugInfo` trait bridges gimli sections into object files
- Map each MIR instruction back to a `.rk` source span
- Emit type DIEs (`DW_TAG_base_type`, `DW_TAG_structure_type`) for Rask types

**Reference implementation:** `rustc_codegen_cranelift` already does this for Rust on nightly. Line-level debugging works; type-level is partial.

**What it unlocks:** Breakpoints, stepping, stack traces, variable inspection in any GDB/LLDB-based tool — including VSCode's codelldb extension.

### D2. Time-travel debugging via rr

Once DWARF works, `rr record ./program && rr replay` gives reverse execution for free. No Rask-side work beyond D1.

**Constraints:**
- Linux only, x86_64
- Serializes multi-threaded programs to one core (~1.2x overhead single-threaded, more for multi-threaded)
- Needs `perf_event_paranoid <= 1`

---

## Tier 2 — Rask-Aware Debugging

These features exploit what the compiler knows that generic debuggers don't.

### D3. Pool inspector

Show all live entities in a pool as a table. Highlight which handles point where. Flag generation mismatches (stale handles).

```
Pool<Enemy> (id=2, capacity=64, live=12)
  [0] gen=3  health=45  pos=(10,20)  ← handle(2,0,3)
  [1] gen=1  [free]
  [2] gen=5  health=100 pos=(0,0)
  ...
```

I think this is the highest-value Rask-specific feature. Every ECS game engine builds a custom version of this. Rask could ship it.

### D4. Channel buffer view

Show queued messages in each channel, who's blocked sending/receiving, buffer capacity.

```
Channel<Event> (cap=16, queued=3)
  [0] Click(pos=(5,10))
  [1] Key('a')
  [2] Resize(800,600)
  Blocked receivers: task#4
```

### D5. Ownership visualization

At any breakpoint, show who owns what. The compiler tracks this — surface it in the debugger.

### D6. `using` context stack

Show active capabilities and where they were introduced:

```
Active contexts:
  Pool<Player>     from game.rk:12
  Pool<Enemy>      from game.rk:13
  Multitasking     from main.rk:5
```

### D7. State diffing between stops

Instead of "what is the state?", show "what changed since last breakpoint?"

```
Stopped at game_loop.rk:52  (3rd iteration)
  Changed:
    pool[player_handle].position: (10, 20) → (11, 20)
    pool[enemy_handle].health: 50 → 45
    score: 100 → 110
  Unchanged: 847 other values
```

---

## Tier 3 — Advanced

### D8. Omniscient pool debugging (query the past)

Instrument pool operations at compile time. Record a structured event log. Query it after execution:

```
> query handle(pool=0, index=5) mutations
  t=1042  health: 100 → 90   at game_loop.rk:47
  t=1185  health: 90 → 75    at game_loop.rk:47
  t=1301  health: 75 → 0     at damage.rk:12
  t=1302  [recycled]          at pool.rk:88
```

This falls out naturally from Rask's pool model. In C++ you'd need sanitizer-level instrumentation. In Rask, every pool mutation goes through `pool[handle]` — the compiler knows every access point.

**Overhead estimate:** ~1-2% for dev builds (ring buffer append per pool operation). Acceptable to leave on permanently in debug mode.

### D9. Conditional time-travel

Combine time-travel with assertions — binary-search the execution timeline:

```
> watch pool[h].health < 0
  First violation at t=1301, damage.rk:12
  [jumps to that point in time]
```

Like `git bisect` for execution time. Requires D2 (rr) + D8 (instrumentation).

### D10. Hot-patch debugging (edit-and-continue)

Change a function body while the program is running, continue with the new code.

Cranelift's `cranelift-jit` has an explicit hotswap API: `JITBuilder::hotswap(true)` enables GOT-based indirection, `JITModule::prepare_for_function_redefine()` allows recompiling individual functions.

**Constraints:**
- Only function bodies, not signatures or struct layouts
- Requires PIC codegen (slight overhead)
- The hard part is incremental compilation in the Rask pipeline, not the Cranelift side

### D11. Replay-based test generation

When the debugger catches a bug, automatically extract a minimal reproducing test from the recorded execution trace.

---

## Implementation Path

**Phase 1:** D1 (DWARF emission). This is the foundation — everything else builds on it.

**Phase 2:** D3-D7 (Rask-aware features). Implement as a custom DAP server that wraps LLDB and adds semantic understanding of pools, channels, ownership.

**Phase 3:** D8-D11 (advanced features). Requires compile-time instrumentation and deeper integration.

## Feasibility Notes

| Feature | Feasibility | Key Risk |
|---------|-------------|----------|
| D1 DWARF | Proven (rustc_codegen_cranelift does it) | Maintaining source maps through MIR lowering |
| D2 rr | Just works with D1 | Linux-only |
| D3-D6 Inspectors | Custom DAP server, medium effort | DAP crates are immature (dap-rs 0.4.1-alpha) |
| D7 State diff | Needs D1 + memory snapshots | Snapshot overhead for large state |
| D8 Omniscient | Low instrumentation overhead (~2%) | Trace storage for long-running programs |
| D9 Conditional TT | Requires D2 + D8 | Composing rr with custom traces |
| D10 Hot-patch | Cranelift API exists | Incremental compilation in Rask pipeline |
| D11 Test gen | Research-grade | Minimizing traces to readable tests |
