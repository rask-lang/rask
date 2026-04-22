<!-- id: comp.hidden-params -->
<!-- status: decided -->
<!-- summary: Compiler pass that inserts hidden context parameters for Pool using clauses -->
<!-- depends: memory/context-clauses.md -->

# Hidden Parameter Compiler Pass

Compiler pass that desugars `using Pool<T>` clauses into hidden function parameters. Runs after type checking, before MIR lowering. Scoped to `Pool<T>` contexts only.

**Not in this pass:** `using Multitasking` and `using ThreadPool` do NOT thread hidden parameters. They lower directly into process-global slot install/uninstall calls (`conc.runtime`). Functions do not declare `using Multitasking` on signatures — the compiler instead infers which functions transitively reach `spawn` and enforces the scope at call sites.

## Pass Overview

| Rule | Description |
|------|-------------|
| **HP1: Position in pipeline** | Runs after type checking, before monomorphization and MIR lowering |
| **HP2: Pool contexts only** | Only `using Pool<T>` (and named/frozen variants) desugar through this pass. Runtime/ThreadPool contexts use the process-global slot model in `conc.runtime` |
| **HP3: Three operations** | The pass does three things: (1) rewrite function signatures, (2) rewrite call sites, (3) propagate through closures |
| **HP4: Idempotent** | Running the pass twice produces the same output. No double-insertion of parameters |

```
Source → Lexer → Parser → AST
  → Resolver → TypeChecker → [Hidden Param Pass] → Monomorphize → MIR → Codegen
```

## Pass Inputs and Outputs

**Input:** Typed AST with:
- Functions annotated with `using Pool<T>` clauses (CC1-CC3 from `mem.context`)
- Type information for all expressions (needed for context resolution)

**Output:** Desugared AST where:
- Pool `using` clauses replaced with explicit hidden parameters
- Call sites have hidden arguments inserted
- Closures capture pool contexts appropriately

`using Multitasking { ... }` and `using ThreadPool { ... }` blocks are NOT rewritten by this pass. They are lowered directly by MIR lowering to install/uninstall calls against the process-global runtime slot — see `conc.runtime`.

## Step 1: Rewrite Function Signatures

| Rule | Description |
|------|-------------|
| **SIG1: Pool context → parameter** | `func f() using Pool<T>` becomes `func f(__ctx_pool_T: &Pool<T>)` |
| **SIG2: Named pool → parameter** | `func f() using players: Pool<T>` becomes `func f(__ctx_players: &Pool<T>)` with local alias |
| **SIG3: Frozen pool → const ref** | `func f() using frozen Pool<T>` becomes `func f(__ctx_pool_T: &Pool<T>)` (read-only enforced by type checker) |
| **SIG5: Multiple pool contexts** | Each `using Pool<T>` clause becomes one hidden parameter |
| **SIG6: Hidden param naming** | `__ctx_` prefix marks hidden params |

### Examples

<!-- test: skip -->
```rask
// Before: pool context
func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
    h.health -= amount
}

// After: hidden parameter
func damage(h: Handle<Player>, amount: i32, __ctx_pool_Player: &Pool<Player>) {
    __ctx_pool_Player[h].health -= amount
}
```

<!-- test: skip -->
```rask
// Before: named pool context
func award_bonus(h: Handle<Player>, amount: i32) using players: Pool<Player> {
    h.score += amount
    players.mark_dirty(h)
}

// After: hidden parameter with local alias
func award_bonus(h: Handle<Player>, amount: i32, __ctx_players: &Pool<Player>) {
    const players = __ctx_players  // Local alias for named context
    __ctx_players[h].score += amount
    players.mark_dirty(h)
}
```

### Parameter typing

| Context type | Parameter type | Rationale |
|-------------|---------------|-----------|
| `using Pool<T>` | `&Pool<T>` (required) | Pool must exist — compile error if not available |

Pool contexts are required because `Handle<T>` field access doesn't work without a pool.

**Runtime context** is NOT threaded as a hidden parameter. Functions that perform I/O or `spawn` read from the process-global runtime slot (installed by `using Multitasking { ... }`) at the call site. See `conc.runtime` for the slot model and `conc.async/CC1-CC3` for the static scope check.

## Step 2: Rewrite Call Sites

| Rule | Description |
|------|-------------|
| **CALL1: Resolve context source** | At each call to a function with hidden params, find the context value in scope |
| **CALL2: Resolution order** | Search (in order): local variables → function parameters → fields of `self` → own `using` clause (same as `mem.context/CC4`) |
| **CALL3: Insert hidden argument** | Append resolved context value as hidden argument at call site |
| **CALL4: Propagation** | If the caller also has a `using` clause for the same type, its hidden parameter satisfies the callee's requirement (`mem.context/CC5`) |
| **CALL5: Ambiguity is error** | Multiple pools of same type in scope → compile error (`mem.context/CC8`) |

### Resolution algorithm

```
resolve_context(call_site, required_type) -> ContextSource:
    // 1. Local variables in current scope
    for var in current_scope.locals:
        if var.type matches required_type:
            return LocalVar(var)

    // 2. Function parameters (explicit)
    for param in current_function.params:
        if param.type matches required_type:
            return Param(param)

    // 3. Fields of self (if in a method)
    if current_function.has_self:
        for field in self.type.fields:
            if field.type matches required_type:
                return SelfField(field)

    // 4. Own hidden context parameter (propagation)
    for hidden in current_function.hidden_params:
        if hidden.type matches required_type:
            return HiddenParam(hidden)

    // 5. Not found
    error("no context of type {required_type} available at {call_site}")
```

### Example: call site rewriting

<!-- test: skip -->
```rask
// Before:
func game_tick() {
    const players = Pool.new()
    const h = players.insert(Player.new())
    damage(h, 10)    // How does damage() get the pool?
}

// After:
func game_tick() {
    const players = Pool.new()
    const h = players.insert(Player.new())
    damage(h, 10, &players)    // Resolved: local variable `players`
}
```

<!-- test: skip -->
```rask
// Before: propagation through call chain
func update_player(h: Handle<Player>) using Pool<Player> {
    take_damage(h, 5)
    check_death(h)
}

// After:
func update_player(h: Handle<Player>, __ctx_pool_Player: &Pool<Player>) {
    take_damage(h, 5, __ctx_pool_Player)    // Propagated from own hidden param
    check_death(h, __ctx_pool_Player)       // Same
}
```

## Step 3: `using Multitasking` / `using ThreadPool` Blocks

These blocks are NOT handled by this pass. They are lowered by MIR lowering (see `conc.runtime`) into direct install/uninstall calls against a process-global slot:

```rask
// Source:
using Multitasking(workers: 4) {
    body
}

// Lowered (conceptual):
__runtime_enter(RuntimeConfig { workers: 4 })  // panics if a block is already active
{
    body
}
__runtime_exit()  // drains all tasks, clears the slot
```

No hidden parameters, no propagation through callees. `spawn()` and stdlib I/O read the slot directly when invoked.

## Step 4: Closure Pool Context Capture

| Rule | Description |
|------|-------------|
| **CL1: Immediate closures inherit** | Expression-scoped closures (iterator callbacks, immediate callbacks) capture pool contexts by reference (`mem.context/CC9`) |
| **CL3: Storable closures exclude** | Storable closures cannot capture pool contexts implicitly (`mem.context/CC10`) |
| **CL4: Pool context in spawn** | Spawn closures can capture pool contexts if the pool is `Send + Sync`. Runtime context is NOT captured — spawn'd tasks read the process-global slot when they execute on a worker thread (see `conc.runtime`) |

### Iterator closure desugaring

<!-- test: skip -->
```rask
// Before:
func process_all(handles: Vec<Handle<Player>>) using Pool<Player> {
    for h in handles {
        h.score += 10
    }
}

// After:
func process_all(handles: Vec<Handle<Player>>, __ctx_pool_Player: &Pool<Player>) {
    for h in handles {
        // __ctx_pool_Player captured by reference (expression-scoped)
        __ctx_pool_Player[h].score += 10
    }
}
```

## Implementation Notes

### Pass structure (Rust pseudocode)

```rust
struct HiddenParamPass {
    // Track which functions need which hidden params
    function_contexts: HashMap<FuncId, Vec<HiddenParam>>,
}

struct HiddenParam {
    name: String,          // __ctx_pool_Player, __ctx_players
    param_type: Type,      // &Pool<Player>
    source: ContextSource, // Where it comes from at call sites
}

impl HiddenParamPass {
    fn run(ast: &mut TypedProgram) {
        // Phase 1: Collect — scan all functions for `using` clauses
        self.collect_contexts(ast);

        // Phase 2: Propagate — functions that call context-needing functions
        //          also need the context (transitive)
        self.propagate_contexts(ast);

        // Phase 3: Rewrite signatures — add hidden params
        self.rewrite_signatures(ast);

        // Phase 4: Rewrite call sites — insert hidden arguments
        self.rewrite_calls(ast);

        // Phase 5: Rewrite using blocks — construct/teardown
        self.rewrite_blocks(ast);

        // Phase 6: Rewrite closures — capture rules
        self.rewrite_closures(ast);
    }
}
```

### Propagation algorithm

The critical subtlety: functions that don't declare `using` clauses but call functions that require context must also receive the hidden parameter.

```
propagate_contexts(call_graph):
    changed = true
    while changed:
        changed = false
        for func in all_functions:
            for callee in func.callees:
                for ctx in callee.required_contexts:
                    if ctx not in func.required_contexts:
                        if func can resolve ctx from locals/self/params:
                            // Context available locally, no propagation needed
                            continue
                        else:
                            // Must propagate: add hidden param to func
                            func.required_contexts.add(ctx)
                            changed = true
```

**Fixed-point iteration:** Keep propagating until no new contexts added. Typically converges in 2-3 iterations (call chains are shallow).

**Cycle handling:** Recursive functions that need context propagate it in one iteration (function already has it from the recursive call requirement).

### Public function constraint

| Rule | Description |
|------|-------------|
| **PUB1: Public functions declare contexts** | Public functions must have explicit `using` clauses (`mem.context/CC6`). The pass does not add hidden params to public functions that don't declare them |
| **PUB2: Private functions may infer** | Private functions can have contexts inferred by propagation (`mem.context/CC7`) |

This prevents context propagation from changing public API surfaces. If a private helper needs a pool, propagation adds it silently. If a public function needs a pool, the programmer must declare it.

## Interaction with Monomorphization

| Rule | Description |
|------|-------------|
| **MONO1: Before monomorphization** | Hidden param pass runs before monomorphization. Generic functions get generic hidden params |
| **MONO2: Specialized contexts** | `func f<T>() using Pool<T>` becomes `func f<T>(__ctx_pool_T: &Pool<T>)`. Monomorphization then specializes both `T` and the hidden param type |

<!-- test: skip -->
```rask
// Before:
func process_all<T>(handles: Vec<Handle<T>>) using Pool<T>
    where T: Processable
{
    for h in handles { h.process() }
}

// After hidden param pass:
func process_all<T>(handles: Vec<Handle<T>>, __ctx_pool_T: &Pool<T>)
    where T: Processable
{
    for h in handles { __ctx_pool_T[h].process() }
}

// After monomorphization (for T = Player):
func process_all_Player(handles: Vec<Handle<Player>>, __ctx_pool_Player: &Pool<Player>) {
    for h in handles { __ctx_pool_Player[h].process() }
}
```

## Error Messages

```
ERROR [comp.hidden-params/CALL5]: ambiguous context
   |
10 |  const pool_a = Pool::<Player>.new()
11 |  const pool_b = Pool::<Player>.new()
13 |  damage(h, 10)
   |  ^^^^^^^^^^^ multiple Pool<Player> in scope
   |
WHY: The compiler can't determine which pool to pass as hidden context.

FIX: Pass the pool explicitly:
  damage_explicit(pool_a, h, 10)
```

```
ERROR [comp.hidden-params/PUB1]: public function needs explicit context
   |
1  |  public func damage(h: Handle<Player>, amount: i32) {
   |                      ^^^^^^^^^^^^^^^^ uses Handle<Player> but no context declared
   |
WHY: Public functions must declare context dependencies in their signature.

FIX: Add using clause:
  public func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
```

```
ERROR [comp.hidden-params/CL3]: storable closure cannot capture context
   |
5  |  const callback: |Handle<Player>| = |h| {
6  |      h.health -= 10
   |      ^ no Pool<Player> context
   |
WHY: Storable closures may execute where context isn't available.

FIX: Pass the pool as an explicit parameter.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| Function with `using Pool<T>` called outside any pool scope | Compile error | CALL1 |
| Public function without `using` calls private function with context | Compile error on public function | PUB1 |
| Recursive function with `using` clause | Self-propagation, one hidden param | Propagation |
| `comptime` function with `using Pool<T>` | Compile error (no pools at comptime) | `ctrl.comptime/CT20` |
| Generic function with `using Pool<T>` | Hidden param is generic, specialized at monomorphization | MONO2 |
| Closure captures two different pool contexts | Two hidden captures, ordered same as enclosing function | CL1, SIG5 |
| `using Multitasking` inside another `using Multitasking` | Runtime panic (second enter aborts) | `conc.async/C1` |

---

## Appendix (non-normative)

### Rationale

**HP1 (after type checking):** The pass needs type information to resolve contexts (know which variables are `Pool<Player>` vs `Pool<Enemy>`). Running before monomorphization means we handle generics once, not per-instantiation.

**HP2 (pool contexts only):** Pool contexts must thread as hidden params because they are value references that callees actually use. Runtime contexts (`Multitasking`, `ThreadPool`) live in a process-global slot instead — there is one per process by design, every thread sees it, and adding a hidden parameter for it would color every function that transitively reaches `spawn`, violating Principle 5. See `conc.runtime`.

**PUB1 (public functions declare):** Without this rule, adding a private helper that needs a pool could silently change a public function's ABI. Requiring explicit declaration on public functions means ABI changes are intentional and visible in diffs.

### Debugging the pass

`rask check --explicit-context` shows the desugared signatures:

```
$ rask check --explicit-context game.rk

func damage(h: Handle<Player>, amount: i32)
  + hidden: __ctx_pool_Player: &Pool<Player>
  resolved from: local variable 'players' at game.rk:5
```

This helps programmers understand pool context flow when debugging unexpected behavior.

### Why hidden params for pools but a process-global slot for runtime

Pool contexts are values the callee dereferences (`pool[handle].field`) — they must be passed as real references. Threading them as hidden parameters keeps the value flow explicit in the IR and enables the compiler to prove liveness.

Runtime contexts are different: `spawn` and stdlib I/O want to find "the current runtime" — a singleton resource per process. Threading it as a hidden parameter would make every function that transitively reaches `spawn` take an extra argument, coloring signatures up the call graph. The process-global slot (see `conc.runtime`) avoids that: there's exactly one slot, every thread reads it, and functions carry no annotation.

### See Also

- `mem.context/CC1-CC10` — Using clause semantics (programmer-facing)
- `conc.io-context` — How I/O functions use the runtime context
- `conc.runtime/HP1-HP3` — Debuggability requirements for hidden parameters
- `conc.strategy` — Phase A vs Phase B runtime (affects context mode)
- `comp.codegen/P1` — MIR pipeline position
