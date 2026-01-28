# Holy Grail Language Metrics V2.0

**Goal:** Groundbreaking systems language that eliminates abstraction tax while covering 80%+ of real-world use cases.

---

## Core Metrics

### 1. Transparency Coefficient (TC)
```
TC = (Apparent Operations) / (Actual Operations)
Target: TC ≥ 0.90
```

MAJOR costs must be visible. Small O(1) safety checks can be implicit.

**Visible (require explicit syntax):**
- Allocations, reallocations
- Locks, I/O, syscalls
- Large copies

**Implicit OK (don't require ceremony):**
- Bounds checks
- Null/generation checks
- Small safety validations

**Examples:**
- ✅ `arena.users[42]` - clearly base + offset
- ✅ `vec.push(x)` - bounded vec, no realloc possible, check is implicit
- ✅ `entity.target.health` - generation check implicit, not in your face
- ❌ `vec.push(x)` on unbounded vec - hides possible allocation

---

### 2. Mechanical Correctness (MC)
```
MC = (Impossible Bug Classes) / (10 Common Bug Classes)
Target: MC ≥ 0.90

Bug Classes: use-after-free, double-free, data races, null deref,
buffer overflow, memory leaks, stale references, type confusion,
uninitialized reads, integer overflow
```

Bugs must be impossible by construction, not just caught at runtime.

---

### 3. Use Case Coverage (UCC)
```
UCC = Σ (weight × expressibility_score)
Target: UCC ≥ 0.80

Weights:
- Web services:    30%
- CLI tools:       20%
- Data processing: 15%
- Desktop apps:    15%
- Embedded:        10%
- Game engines:     5%
- OS kernels:       5%

Expressibility:
- 1.0 = Natural and idiomatic
- 0.7 = Possible but verbose
- 0.4 = Requires workarounds
- 0.0 = Cannot express
```

**Test: 10 Canonical Programs**
1. HTTP JSON API server
2. grep clone
3. Log aggregation pipeline
4. Text editor
5. Sensor data processor
6. 3D renderer
7. Network packet router
8. Compiler
9. Database
10. Real-time game loop

---

### 4. Predictability Index (PI)
```
PI = % of code where programmers can predict:
  - Memory layout (±20%)
  - Execution time (±1 order of magnitude)
  - Resource usage
  - Failure modes

Target: PI ≥ 0.85
```

**Measure:** User studies with intermediate programmers.

---

### 5. Ergonomic Delta (ED)
```
ED = (Effort in Rask) / (Effort in Best-in-Class)

Compare against WHICHEVER language is simplest for that task:
- Web services: Go, Kotlin, TypeScript
- CLI tools: Python, Ruby, Go
- Data processing: Python, Julia, Elixir
- Systems: Zig, Odin, C3
- Games: Jai, Odin, C#
- Embedded: Zig, C, Forth

Target: ED ≤ 1.2 vs the simplest alternative for that use case
```

**Critical:** Rask must feel natural. Compare against whatever language solves that problem most elegantly—not just Rust/Go.

**Measure via:**
- Lines of code for equivalent functionality
- Number of error handling sites
- Nesting depth for common patterns
- Annotations/declarations required

**Inspiration welcome from:** Swift (optionals), Zig (comptime), Odin (context), Jai (metaprogramming), Vale (regions), Koka (effects), OCaml (inference), Elixir (pipes), Nim (macros)

---

### 6. Syntactic Noise (SN) - NEW
```
SN = (Ceremony tokens) / (Logic tokens)
Target: SN ≤ 0.3 (at most 30% overhead)

Ceremony: error handling, type annotations, lifetime markers,
          capacity declarations, explicit unwrapping
Logic: actual computation, data transformation, control flow
```

**Examples:**
- ✅ `users.get(id)?.name` — minimal ceremony
- ❌ `users.get(id) or { return None }.name or { return None }` — ceremony dominates
- ✅ `for user in users { ... }` — clean iteration
- ❌ `users.lock(ref) { |user| ... } or continue` — nested callback noise

**Red flags (auto-fail if common patterns require):**
- `or` clause on >50% of lines
- Nesting depth >2 for single operations
- Error handling longer than happy path

---

### 7. Innovation Factor (IF)

**Qualitative:** Must enable patterns impossible or 10× harder in existing languages.

---

### 8. Runtime Overhead (RO)
```
RO = (Rask runtime cost) / (Equivalent C/Rust cost)
Target: RO ≤ 1.10 for hot paths
        RO ≤ 1.50 for cold paths
```

Zero-cost abstractions preferred. Runtime costs must be **opt-in**.

**Acceptable (implicit):** bounds checks, null checks, generation checks

**Must be opt-in:** GC, RC, deep copies, runtime capability checks

**Red flags (auto-fail):**
- Mandatory GC/RC for all allocations
- No zero-copy path for message passing
- Hidden allocations in basic operations

---

### 9. Compilation Speed (CS)
```
CS = (Rust compile time) / (Rask compile time)
Target: CS ≥ 5× faster than Rust
```

No whole-program analysis. Module-local inference only.

**Red flags (auto-fail):**
- Whole-program escape/borrow analysis
- Compile time growing superlinearly with codebase size

---

## Success Criteria

```
TC ≥ 0.90 AND MC ≥ 0.90 AND UCC ≥ 0.80 AND PI ≥ 0.85 AND ED ≤ 1.2 AND SN ≤ 0.3 AND RO ≤ 1.10 (hot) AND CS ≥ 5× Rust AND IF is HIGH
```

**Hard requirement:** Must feel lighter than the status quo for systems programming.

**The goal:** A language where safety is the default but doesn't feel like a tax. Draw inspiration from ANYWHERE—the best ideas often come from unexpected sources (Erlang's supervision, Forth's simplicity, APL's notation, Lisp's macros, ML's types).

**Not bound to:** Rust's borrow checker, Go's simplicity, C's model. These are data points, not constraints.

**The holy grail:** Memory safety that doesn't feel like memory safety. The programmer thinks about their problem, not the language.

---