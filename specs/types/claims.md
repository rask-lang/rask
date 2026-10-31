<!-- id: type.claims -->
<!-- status: proposed -->
<!-- summary: Hand-written statements of what code must do, run under sim over generated inputs at build time; a counterexample fails the build -->
<!-- depends: sim.md, determinism.md, stdlib/testing.md, types/generics.md, stdlib/encoding.md -->
<!-- implemented-by: compiler/crates/rask-cli/src/commands/claims.rs, compiler/runtime/claims.c, stdlib/claims.rk, compiler/crates/rask-mir/src/transform/claim_steps.rs -->

# Claims

A claim says what code must do for every input. A person writes it, the build checks it, and an input that breaks it fails the build.

<!-- test: skip -->
```rask
claim sort_orders(v: Vec<i64>) {
    let s = sort(v)
    for i in 1..s.len() {
        assert s[i - 1] <= s[i]
    }
}
```

The reason is who writes the code now. When a model writes the code and also the tests, the tests show that the two agree, and that's all they show. Nothing ties either one to what the program is for. A claim is the operator's statement of intent, kept apart from the code, and the compiler checks the code against it deterministically. An agent that breaks a claim gets a build failure, not a red test it can rewrite.

I want this without turning Rask into a proof language. Nobody writes a proof. The compiler checks as hard as it can and reports how far it got.

A claim checks what it says and nothing more. A package held to ten claims has ten things checked, not a proof that it's correct.

> **Built so far.** A package's `claims.rk` (W1), checked by `rask build`,
> `rask run`, `rask test` and `rask check --claims`; `rask check` type-checks
> it (R1). Inputs are built for anything `json.decode` builds, plus `char`,
> tuples and enums (IN1). Exhaustive (V2) and sampled (V3) verdicts, shrinking
> (V1), the step budget (D2, `@claim_budget`) and the seed (D1) work. Not yet:
> shared `[claims]` sources (W2, W3), interface claims (G1, G2), cached
> verdicts and calls (D4, D5), strict mode (V6), hand-written generators
> (IN2), the `rask claims` tool, and inputs of 128-bit integers, `T or E` and
> maps with non-string keys.

## Declaration

| Rule | Description |
|------|-------------|
| **CL1: Shape** | `claim name(params) { body }`. The parameters are the inputs the claim holds for, all of them. The body checks with `assert` and `check` (`std.testing/A1`, `A2`) and returns nothing |
| **CL2: Claims sources only** | A claim is declared only in a claims source: a package's `claims.rk` (W1) or a file or directory its manifest lists under `[claims]` (W2). `claim` anywhere else is a compile error |
| **CL3: Nothing depends on a claims source** | A claims source may also hold helper functions (a reference model, a generator). Code can't import it; it's reached only through `[claims]` (W2), and release builds strip it, like tests |
| **CL4: Runs under sim** | A claim is compiled to native code and run on the sim runtime (`sim`): one thread at a time, with scheduling, the clock, I/O and faults all drawn from a seed. So a claim can do what a sim test can, `spawn` and I/O included. Each input starts in a fresh world: no tasks, no files written, no sockets. What sim refuses (`Thread.spawn`, C that escapes to the real world, `sim/B1`, `B2`) a claim can't reach either |
| **CL5: Discarding an input** | A `return` before the end of the body discards that input. It counts neither for nor against the claim. That's how a claim states a precondition: `if !is_valid(x) { return }` |
| **CL6: A panic is a failure** | If the code under the claim panics on an input (out of bounds, overflow, `unreachable()`), that input is a counterexample. A claim that just calls a function claims it never panics |

CL2 is the load-bearing rule. A claim only works if the person who wrote it isn't the one who wrote the code. If an agent can edit the claim, its cheapest way past a failed build is to weaken the claim. Keeping claims in their own files is what lets a repo protect them (code owners on one path, or a separate repo with its own access) and lets a reviewer see a claim change on its own. The language can't enforce who wrote a file. It can make claims separable, so the repo can.

A claims source is compiled as its own package, even a file sitting in the package directory, so it sees only what the package it checks makes `public` (`struct.modules`). Claims state what a package promises, and its internals can be rewritten freely as long as the promise holds. That also keeps claims few and broad: checking individual functions is what tests are for.

## Where claims live

A package's own claims sit in one reserved file. A repo's, a company's, and an outside author's are listed in the manifest, and they differ only in where they come from.

| Rule | Description |
|------|-------------|
| **W1: `claims.rk` is reserved** | `claims.rk` in a package directory is always that package's claims, with nothing to configure, the way `build.rk` is always its build script. It's taken out of the package's own sources (an exception to `struct.modules/PO1`) and compiled as its own package. A package without a manifest still gets one |
| **W2: Shared claims** | A package lists further claims sources it's held to under `[claims]`, each a file or a directory of claim files, wherever the repo or company keeps them. Every claim in `claims.rk` and in the listed sources is checked in that package's build: a claim that imports the package checks its public API, and a generic claim (G1) checks its conforming types. `[claims]` isn't a code dependency, so a claims source importing the package that applies it is not a cycle |
| **W3: Locked like dependencies** | A claims source from a registry or git is pinned in `rask.lock` with its checksum (`struct.packages/LK3`). Editing it locally fails the build, so the only way to change one is a visible lock update |

<!-- test: skip -->
```toml
[claims]
repo = { path = "../shared-claims" }                                # repo-wide
acme = { git = "https://git.acme.dev/claims", version = "2.1" }     # company-wide
```

The stdlib's interface laws are a claims source every package is held to without listing it.

## Inputs

| Rule | Description |
|------|-------------|
| **IN1: Built like a decode** | A claim's inputs are the values `json.decode` could build, so a parameter's type must be `Decode` (`std.encoding/E12`–`E17`), and `char`, tuples and enums are built too. Both walk one description of the type, so a field the wire form leaves out takes its default here as it does in a decode (`E13a`). A type a decode can't build is made inside the claim instead, through the package's API, usually by applying a generated list of operations |
| **IN2: Own generator** | A type with an invariant its fields don't express (a sorted `Vec`, a valid AST) supplies its own generator in a claim file. Spelling open, see below |
| **IN3: Not generatable in v1** | `@resource` types, `Shared`, `Rack`/`Link`, and `any I` can't be claim parameters. A claim can still build them in its body from generated parts |

## Verdicts

Every claim ends with a verdict. Only a counterexample fails the build.

| Rule | Description |
|------|-------------|
| **V1: Counterexample** | Some input breaks the claim. The build fails, showing the smallest input the checker could shrink it to and the failed assertion with both operands (`std.testing/A5`). A claim that reached a scheduling point or simulated I/O also gets sim's replay line (`sim/I4`) |
| **V2: Checked all** | Every input in the claim's domain was tried. Applies when every parameter's domain is finite and the product is at most 65,536 (bools, enums without payloads, `u8`, `i8`, and tuples and structs of those), and the claim never reaches a scheduling point or simulated I/O. Within that domain, the claim is proven. Once a claim touches either, the schedule and faults are sampled too, and the verdict is at most V3 |
| **V3: Sampled** | The claim held for N generated inputs (default 1,000). Reported, not an error. The verdict says the number and never rounds up to proven |
| **V4: Proven** | Reserved for a solver backend, for claims that touch no scheduling point or I/O. Same claims, stronger verdict. Not part of v1 |
| **V5: Out of budget** | An input ran past the claim's step budget. Reported with the input, not an error. Not finishing isn't the same as being shown false |
| **V6: Strict mode** | `--claims=strict` turns every verdict except checked all and proven into an error. For CI that wants the stronger guarantee. Off by default |

```
claims: game (held to game/claims)
  replay_never_wins   held for 1,000 sampled inputs
  cell_roundtrip      checked all 512 inputs

error[E0950]: claim `sort_is_stable` is false
  --> sorting/claims/order.rk:12:5
   |
12 |     assert s[0].tag == "b"
   |     ^^^^^^^^^^^^^^^^^^^^^^ assertion failed: "a" == "b" (left: "a", right: "b")
   |
   = counterexample: items = [Item(1, "b"), Item(1, "a")]
   = the claim is in sorting/claims/order.rk; if it is wrong, fix it there, in review
```

The error code is a placeholder.

## Determinism

A build failure that comes and goes between builds is worse than none, so checking is a pure function of the source and the toolchain.

| Rule | Description |
|------|-------------|
| **D1: Seed from the claim** | The claim's seed derives from its name and the tokens of its body. Each input's sim seed derives from (claim seed, input index), so the inputs, schedules and faults are the same every build. Editing the claim draws new ones |
| **D2: Step budget** | Each input gets a budget counted in steps, never wall-clock time. A claim build counts loop back-edges and calls; ordinary builds don't carry the counter. `@claim_budget(N)` on a claim raises it |
| **D3: Toolchain-pinned** | A new compiler may change the generators and draw different inputs. That's a toolchain change like any other, and it can turn up a counterexample nobody saw before. That's the point of it |
| **D4: Cached verdicts** | A verdict is cached under the claim's text and the semantic hashes of every function it reaches (`comp.architecture/IC2`). A claim is re-checked only when something it reaches changed, so a large claim costs once per change, not once per build |
| **D5: Cached calls** | Calls a claim makes into the package it checks are memoized when the effects analysis finds them pure (no I/O, no scheduling point); an effectful call's result also depends on the sim state, not only its arguments. Pure calls are memoized on (the callee's semantic hash, the receiver and argument values), and the result holds the returned value and, for a `mutate` parameter, the value after the call. A pure call with the same inputs always gives the same result, and owned values mean nothing else can change behind it. Sampled operation lists share prefixes, so the state after `[Deposit(5), Withdraw(2)]` is computed once for every input that starts that way. The memo table is stored with the verdict cache (D4), keyed the same way, so it survives between builds until the callee changes |

Deep random search belongs outside the build, where flaky-on-a-new-seed is expected and the replay line handles it (`sim/I4`). `--claim-seeds N` runs every sampled claim on N more seeds after the build's own. A claim checked in full has nothing left to find and is skipped. The flag goes on the command line, never in a manifest, so what a plain build decides doesn't change.

## When claims run

| Rule | Description |
|------|-------------|
| **R1: Where code is compiled** | Claims run as part of `rask build`, `rask run` and `rask test`, and a counterexample stops it. `rask check` type-checks claims but doesn't run them, since it generates no code. `rask check --claims` builds and runs them |
| **R2: Debug profile** | A claim runs against a debug-profile build of the package, whatever profile is being built, so overflow and bounds checks are on (CL6) |
| **R3: Native, not interpreted** | Claims check the code that ships. A codegen bug that breaks a claim is reported as a counterexample, and it is one |

Unchanged claims cost nothing to re-run (D4), so the build pays only for claims whose code changed.

## Claims on interfaces

A generic claim is checked against every conforming type in the build.

<!-- test: skip -->
```rask
// a shared claims source (W2); the stdlib's applies everywhere
claim antisymmetric<T: Comparable>(a: T, b: T) {
    if a.compare(b) == Less {
        assert b.compare(a) == Greater
    }
}
```

| Rule | Description |
|------|-------------|
| **G1: Per conformance** | A claim with an interface bound is instantiated for each type in the applying package (W2) that conforms to the bound and is generatable (IN1). Each instantiation gets its own verdict |
| **G2: Across packages** | A generic claim applied through W2 reaches every conformance in the applying package, whichever package declared the interface |

Finding every conformance is whole-program knowledge, which `std.reflect` refuses to hand user code (R2). Here the checker does it, the way monomorphization finds instantiations, and user code never sees the list.

Interfaces are nominal because a conformance is a statement about meaning (`type.generics`). Having a `compare()` method doesn't make a type a total order, and G1 is how the compiler holds a conformance to what it says. A person writes the law once, and every type anyone writes is checked against it.

## Weak claims

A hand-written claim can still be weaker than its author thinks. `rask claims --mutate` makes small changes to the functions a claim reaches (flips a comparison, drops a branch, swaps two arguments) and reruns the claim. A claim that holds after every change isn't constraining that code, and it's reported. This is too slow for every build, so it's a tool and never part of one.

## Tooling

| Command | What it does |
|---------|--------------|
| `rask claims <path>` | Lists every claim with its verdict. The surface a reviewer reads, like `rask api` |
| `rask claims --mutate <path>` | Weak-claim report (above) |
| `rask claims --since <rev>` | Lists commits since `<rev>` that changed a claim *and* a function that claim reaches. That's what it looks like when someone rewrites the grader along with the work |

## What a claim can and can't say

A claim can say anything that's a check over values: round trips (`decode(encode(x)) == x`), invariants that survive any sequence of operations, agreement between a fast implementation and a slow obvious one, interface laws, and "never panics". Under sim it can also say things about I/O and concurrency: a server answers every request it accepts, under any schedule and with peers dropping out; a cache stays consistent whatever order its tasks run in. Those verdicts are sampled only (V2), because the schedules and faults are sampled.

It can't say anything about the real world: real files, real networks, real time. Sim stands in for all three. Performance ("runs in O(n)") is out, and so is intent that isn't computable ("the error message is clear"). Those belong in tests, benchmarks and review.

Two cases are weak rather than impossible. Floats need tolerances, because NaN breaks every equality law. Inputs that must satisfy a strong precondition need their own generator (IN2), because random sampling discards nearly everything under CL5.

## Claims and tests

| | Test | Claim |
|---|---|---|
| Inputs | The ones the author picked | Every input of the parameter types |
| Written by | Anyone, agents included | Operators, curated in review |
| Runs under | `rask test` | Every build |
| On failure | Red test | Build fails |
| Can do I/O, spawn | Yes, real or under sim | Under sim only |
| How many | Lots | Few. The program is built around them |

## Open

- **Generator spelling** (IN2). An interface the type conforms to in its claim file is the likely shape. The name isn't picked.
- **Solver backend** (V4). Rask's memory model is what makes this tractable. With no storable references, single ownership, and mutable borrows that exclude everything else, a prover rarely has to ask what else a write changed. Loops are the hard part. A proof needs an invariant, and when the solver can't infer one the verdict falls back to V2 or V3. Nobody is asked to write one, and whether they *may* is open.
- **Function contracts.** A guarantee written on a function signature would read better than a separate claim for simple cases. `ensure` is taken (cleanup), so contracts would need their own word, and they would break CL2 by putting intent next to code an agent edits.
- **Is public-only too strict?** Hard to tell before real packages use it. The sign would be packages making things `public` only so their own claims can reach them (an `is_balanced()` nobody else calls). If that shows up, the likely answer is private access for the package's own `claims.rk`, and only that.
- **Claims over functions.** A shared claim can quantify over types (G1) but not over functions, so "no public function panics" can't be written once for everyone. That needs reflection over a package's functions, which doesn't exist.
- **Manifest section.** `[claims]` is a new manifest section and goes through the schema's change history (`struct.manifest`).
