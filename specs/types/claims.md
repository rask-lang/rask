<!-- id: type.claims -->
<!-- status: proposed -->
<!-- summary: Hand-written statements of what code must do, checked by the compiler over generated inputs; a counterexample is a compile error -->
<!-- depends: control/comptime.md, stdlib/testing.md, types/generics.md, stdlib/encoding.md -->

# Claims

A claim says what code must do for every input. A person writes it, the compiler checks it, and an input that breaks it is a compile error.

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

## Declaration

| Rule | Description |
|------|-------------|
| **CL1: Shape** | `claim name(params) { body }`. The parameters are the inputs the claim holds for, all of them. The body checks with `assert` and `check` (`std.testing/A1`, `A2`) and returns nothing |
| **CL2: Claims sources only** | A claim is declared only in a claims source: a file or directory some manifest lists under `[claims]` (W1). Being listed is what makes it one. `claim` anywhere else is a compile error |
| **CL3: Nothing depends on a claims source** | A claims source may also hold helper functions (a reference model, a generator). Code can't import it; it's reached only through `[claims]` (W2), and release builds strip it, like tests |
| **CL4: Comptime body** | A claim runs in the comptime interpreter. Its body and everything it calls must be comptime-evaluable (`ctrl.comptime/CT6`, `CT7`): no I/O, no `spawn`, no `any I` |
| **CL5: Discarding an input** | A `return` before the end of the body discards that input. It counts neither for nor against the claim. That's how a claim states a precondition: `if !is_valid(x) { return }` |
| **CL6: A panic is a failure** | If the code under the claim panics on an input (out of bounds, overflow, `unreachable()`), that input is a counterexample. A claim that just calls a function claims it never panics |

CL2 is the load-bearing rule. A claim only works if the person who wrote it isn't the one who wrote the code. If an agent can edit the claim, its cheapest way past a compile error is to weaken the claim. Keeping claims in their own files is what lets a repo protect them (code owners on one path, or a separate repo with its own access) and lets a reviewer see a claim change on its own. The language can't enforce who wrote a file. It can make claims separable, so the repo can.

A claims source is compiled as its own package, even a file sitting in the package directory, so it sees only what the package it checks makes `public` (`struct.modules`). Claims state what a package promises, and its internals can be rewritten freely as long as the promise holds. That also keeps claims few and broad: checking individual functions is what tests are for.

## Where claims live

One mechanism covers every scope. A package's own claims, a repo's, a company's, and an outside author's are all claims sources, and they differ only in where the source comes from.

| Rule | Description |
|------|-------------|
| **W1: Claims sources** | A `[claims]` entry names a file (`claims.rk`) or a directory of claim files. A listed file inside the package directory is taken out of the package's own sources (an exception to `struct.modules/PO1`) and compiled as its own package. A package's own claims are usually one `claims.rk`. Shared ones live wherever the repo or company keeps them |
| **W2: Applying** | A package lists the claims sources it's held to under `[claims]`. Every claim in them is checked in that package's build: a claim that imports the package checks its public API, and a generic claim (G1) checks its conforming types. `[claims]` isn't a code dependency, so a claims source importing the package that applies it is not a cycle |
| **W3: Locked like dependencies** | A claims source from a registry or git is pinned in `rask.lock` with its checksum (`struct.packages/LK3`). Editing it locally fails the build, so the only way to change one is a visible lock update |

<!-- test: skip -->
```toml
[claims]
own  = { path = "claims.rk" }                                       # this package's promises
repo = { path = "../shared-claims" }                                # repo-wide
acme = { git = "https://git.acme.dev/claims", version = "2.1" }     # company-wide
```

The stdlib's interface laws are a claims source every package is held to without listing it.

## Inputs

| Rule | Description |
|------|-------------|
| **IN1: Generated by type** | Each parameter's type must be generatable. Primitives, `string`, `Vec`, `Map`, tuples, `T?`, `T or E`, and structs and enums whose fields are all generatable get a generator derived automatically, the same way `Encode` is (`std.encoding/E12`, `E17`) |
| **IN2: Own generator** | A type with an invariant its fields don't express (a sorted `Vec`, a valid AST) supplies its own generator in a claim file. Spelling open, see below |
| **IN3: Not generatable in v1** | `@resource` types, `Shared`, `Rack`/`Link`, and `any I` can't be claim parameters. A claim can still build them in its body from generated parts |

## Verdicts

Every claim ends with a verdict. Only a counterexample fails the build.

| Rule | Description |
|------|-------------|
| **V1: Counterexample** | Some input breaks the claim. Compile error showing the smallest input the checker could shrink it to, and the failed assertion with both operands (`std.testing/A5`) |
| **V2: Checked all** | Every input in the claim's domain was tried. Applies when every parameter's domain is finite and the product is at most 65,536 (bools, enums without payloads, `u8`, `i8`, and tuples and structs of those). Within that domain, the claim is proven |
| **V3: Sampled** | The claim held for N generated inputs (default 1,000). Reported, not an error. The verdict says the number and never rounds up to proven |
| **V4: Proven** | Reserved for a solver backend. Same claims, stronger verdict. Not part of v1 |
| **V5: Out of budget** | An input ran past the claim's step budget. Reported with the input, not an error. Not finishing isn't the same as being shown false |
| **V6: Strict mode** | `rask check --claims strict` turns every verdict except checked all and proven into an error. For CI that wants the stronger guarantee. Off by default |

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

A compile error that comes and goes between builds is worse than none, so checking is a pure function of the source and the toolchain.

| Rule | Description |
|------|-------------|
| **D1: Seed from the claim** | The sampling seed derives from the claim's name and the tokens of its body. The same claim on the same compiler gets the same inputs every time. Editing the claim draws new inputs |
| **D2: Step budget** | Each input gets a budget counted in interpreter steps, never wall-clock time. Claims don't use the comptime branch quota (`ctrl.comptime/CT35`, 1,000), which is sized for constants, not for running a function a thousand times. `@claim_budget(N)` on a claim raises it |
| **D3: Toolchain-pinned** | A new compiler may change the generators and draw different inputs. That's a toolchain change like any other, and it can turn up a counterexample nobody saw before. That's the point of it |
| **D4: Cached verdicts** | A verdict is cached under the claim's text and the semantic hashes of every function it reaches (`comp.architecture/IC2`). A claim is re-checked only when something it reaches changed, so a large claim costs once per change, not once per build |

Deep random search belongs in `rask test`, where flaky-on-a-new-seed is expected and the replay line handles it (`sim/I4`). `rask test --claims --seeds N` runs every claim on N more seeds without changing what `rask check` decides.

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

A hand-written claim can still be weaker than its author thinks. `rask claims --mutate` makes small changes to the functions a claim reaches (flips a comparison, drops a branch, swaps two arguments) and reruns the claim. A claim that holds after every change isn't constraining that code, and it's reported. This is too slow for every build, so it's a tool and never part of `rask check`.

## Tooling

| Command | What it does |
|---------|--------------|
| `rask claims <path>` | Lists every claim with its verdict. The surface a reviewer reads, like `rask api` |
| `rask claims --mutate <path>` | Weak-claim report (above) |
| `rask claims --since <rev>` | Lists commits since `<rev>` that changed a claim *and* a function that claim reaches. That's what it looks like when someone rewrites the grader along with the work |

## What a claim can and can't say

A claim can say anything that's a pure check over values. That covers round trips (`decode(encode(x)) == x`), invariants that survive any sequence of operations, agreement between a fast implementation and a slow obvious one, interface laws, and "never panics".

It can't say anything about I/O, time, or the network, because comptime can't do them (CL4). It can't say anything about concurrent interleavings either, since there's no `spawn` at comptime. Performance ("runs in O(n)") is out, and so is intent that isn't computable ("the error message is clear"). Those belong in tests, benchmarks and review.

Two cases are weak rather than impossible. Floats need tolerances, because NaN breaks every equality law. Inputs that must satisfy a strong precondition need their own generator (IN2), because random sampling discards nearly everything under CL5.

## Claims and tests

| | Test | Claim |
|---|---|---|
| Inputs | The ones the author picked | Every input of the parameter types |
| Written by | Anyone, agents included | Operators, curated in review |
| Runs under | `rask test` | `rask check` |
| On failure | Red test | Compile error |
| Can do I/O, spawn | Yes | No |
| How many | Lots | Few. The program is built around them |

## Open

- **Generator spelling** (IN2). An interface the type conforms to in its claim file is the likely shape. The name isn't picked.
- **Solver backend** (V4). Rask's memory model is what makes this tractable. With no storable references, single ownership, and mutable borrows that exclude everything else, a prover rarely has to ask what else a write changed. Loops are the hard part. A proof needs an invariant, and when the solver can't infer one the verdict falls back to V2 or V3. Nobody is asked to write one, and whether they *may* is open.
- **Function contracts.** A guarantee written on a function signature would read better than a separate claim for simple cases. `ensure` is taken (cleanup), so contracts would need their own word, and they would break CL2 by putting intent next to code an agent edits.
- **Is public-only too strict?** Hard to tell before real packages use it. The sign would be packages making things `public` only so their own claims can reach them (an `is_balanced()` nobody else calls). If that shows up, the likely answer is private access for a claims file listed by the package it checks, and only that one.
- **Claims over functions.** A shared claim can quantify over types (G1) but not over functions, so "no public function panics" can't be written once for everyone. That needs reflection over a package's functions, which doesn't exist.
- **Manifest section.** `[claims]` is a new manifest section and goes through the schema's change history (`struct.manifest`).
- **Claims under sim.** Lifting CL4 for claims run by `rask test --sim` would let claims cover I/O and schedules. Those verdicts would be sampled only.
