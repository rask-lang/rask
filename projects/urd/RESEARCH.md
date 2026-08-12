<!-- id: urd.research -->
<!-- status: draft -->
<!-- summary: Prior art survey and the design decisions Urd inherits from it -->

# Urd — prior art and design notes

What already exists, what each system learned the hard way, and what Urd takes from it. Companion to [README.md](README.md).

## Prior art map

**Datomic** — closest philosophical ancestor: the log is the database, the database is a value, facts accumulate instead of updating places. A decade of production use validates the core idea. What it also shows: users spend most of their time *reading*, so an immutable log without a good read story is a hard sell. Urd needs read functions as a first-class concept, not an afterthought.

**Event sourcing in industry** — the empirical study of production event-sourced systems ([Overeem et al.](https://arxiv.org/abs/2104.01146)) is blunt: the dominant long-term pain is **event schema evolution**, and the tactics that survive are versioned events, weak schema, upcasting, and copy-and-transform. Nobody regrets the log; everybody regrets not planning for ops changing shape. Urd ops carry a version from day 1.

**Temporal / durable execution** — the cautionary tale that matters most. Temporal replays event history against *current* code, so any code change that shifts the command sequence explodes as a nondeterminism error, and the fix is manual versioning APIs threaded through workflow code — its single most hated wart. Urd dissolves this structurally: machine bytecode is content-addressed and stored in the same store as everything else, and **every log entry pins the machine hash it was applied with**. Replay always runs the bytecode that originally ran. Upgrading a machine is an explicit *migration op* in the log — old machine's state in, new machine's state out, itself replayable and verifiable. Code changes can never corrupt history because history remembers which code it was.

**Replicache / Zero** — direct validation of two v1 choices. Their model: named **mutators** (intent + args, not state diffs), server-authoritative execution, client results treated as speculative and **rebased** — unconfirmed mutations replayed on top of new server state, "much like a git rebase." That is exactly Urd's planned merge model, shipping today in production sync engines. One structural improvement Urd gets for free: Replicache needs *two implementations* of every mutator (client JS, server anything) and prays they agree; Urd runs the same content-addressed bytecode on both sides, so speculative and canonical results come from identical code.

**CometBFT / ABCI** — the engine/state-machine split at industrial scale: replication engine on one side, deterministic application on the other, talking through a narrow interface. Three rules worth stealing: state changes *only* via ordered log delivery, never through side channels; the state hash is embedded in the *next* block so replicas must agree; query/read responses may be nondeterministic and are explicitly excluded from agreement. Urd's engine/machine boundary follows the same discipline.

**Factorio / rollback netcode** — deterministic lockstep works (entire genre runs on it) but the practitioner consensus is "endless headaches" — because they build determinism on languages that don't enforce it, then debug desyncs by diffing state dumps. Two takeaways: determinism must be by construction (Raido's job — floats, iteration order, and I/O can't leak in), and desync tooling is a product feature: periodic state hashes in the log, and a `urd verify`-style report that pinpoints the first divergent entry with both states dumped.

**Automerge / Yjs** — history size is the silent killer. Automerge 2 needed ~700 MB of memory for a pasted Moby Dick; [Automerge 3's columnar-compression rewrite](https://automerge.org/blog/automerge-3/) cut that ~100x. Lesson: compact encodings and snapshot/compaction strategy are v1 concerns, not optimizations for later. Also a scope validation: their whole complexity budget goes to automatic merge — which is exactly what Urd defers.

**git** — the storage playbook: content-addressed objects, cheap refs, loose objects compacted into packfiles, shallow clones for clients that don't need deep history. Urd copies it: append-only segments plus a CAS, refs as pointers, loose→packed compaction, and a shallow mode where a client trusts a snapshot hash and only holds history forward from it.

**TigerBeetle / FoundationDB** — the engine itself is a distributed system, and their answer is deterministic simulation testing from day 1: the whole engine runs under a seeded simulator that injects partitions, reorders, and crashes. Urd's engine gets a DST harness before it gets features.

**SQLite** — the packaging model: one file, embedded library, zero config, and a server (in our case) as a second shape of the same core. Nothing to add; just do that.

**Raido's actual competitor class** — deterministic + serializable + metered VMs exist, but only where consensus paid for them: EVM (gas, state hashing, 256-bit words, welded to Ethereum), CosmWasm (deterministic wasm mutators over a host KV store — independently converged on Urd's exact store split), Move (typed, resource-safe, chained to Aptos/Sui), and Agoric's xsnap (Moddable XS JavaScript made deterministic, with resumable heap snapshots — validators compare snapshot hashes). Starlark proves tooling wants hermetic embedded logic but has no state, types, or fuel; the Lua family has neither determinism nor snapshots. Nobody ships the neutral embeddable as a product — the niche is empty because outside consensus nobody needed bit-exactness until replay, sync, and agent auditing. One scar worth keeping: xsnap's heap snapshots diverged across gcc versions (agoric-sdk #7829) — the precise failure design point 10 (hash canonical encoding, never memory) exists to prevent.

## Design implications

Decisions the survey settles:

1. **Log entry** = `{parent_hash, machine_hash, op {name, version, args, recorded_effects}, server_time, seq}`, with a state hash published every N entries. Hash-chained; `verify` re-runs and checks.
2. **Ops are named mutators with typed args** (intent, not diffs). Effects — time, randomness, external data — are recorded fields in the op, stamped by the server at append. Nothing else crosses into the VM.
3. **Machine versioning via content addressing.** Bytecode lives in the CAS; entries pin their machine hash; replay uses the pinned version. Two upgrade paths, because demanding a migration ceremony for a typo fix would be miserable:
   - **Compatible upgrade** (the common case): the new machine can read the current state — a static canonical-encoding/schema comparison, milliseconds. The machine ref advances; no migration op, no ceremony. Typo fixes, refactors, new reads, new op types all land this way. (An earlier draft proved compatibility by replaying the full log — overkill: old entries are pinned to old bytecode forever, so "would history have been identical" is a property nobody needs, at a cost that grows with history. Full replay survives as an optional `--replay` paranoia/CI mode, which is also where deterministic fuel-diff reporting lives.)
   - **Migration op** (the rare case): the state shape actually changes. Old state in, new state out, recorded in the log, verifiable like everything else.

   Pinning is unchanged either way — history always knows which bytecode wrote it. The strictness lives in the chain; the ergonomics live in the tool.
4. **Reads are machine functions** run against current state, ABCI-Query style: allowed to be nondeterministic in formatting, never logged, never part of agreement. Plus whole-state export for small apps and debugging.
5. **Storage engine is boring**: append-only segment files + CAS + refs, git's loose/packed lifecycle. No custom B-tree, no embedded SQL engine.
6. **Compaction and shallow mode from v1**, because history growth killed or nearly killed everyone who ignored it.
7. **Desync tooling is a feature**: periodic state hashes, and a report that binary-searches the log for the first divergent entry.
8. **DST harness for the engine before features.**
9. **Offline writes (v1.5) reuse the Replicache rebase model** — speculative ops client-side, rebase on server order — with one implementation instead of two, once Raido runs in wasm.
10. **State lives in the host store, not the VM — from day 1.** Raido is for logic and scripts, not storage; it was sized for many tiny short-lived scripts, not one immortal gigabyte-scale state. So Urd follows the Cosmos pattern outright: a deterministic hash-tree keyed store in the Rask host (pools and handles doing what they're for), exposed to machines as typed `@table` accessors — the one blessed extern API. The store mutates in place and hashes incrementally; its merkle root is the state hash. State hashes always cover the **canonical encoding**, never anyone's memory layout — which is what lets store implementations evolve without invalidating logs, and makes optional-field additions hash-neutral (canonical encoding omits None — the weak-schema tactic, mechanized).

## Stress test findings

Deliberate attempt to break the design (ergonomics, learning curve, Raido fit). What survived is above; what needs work:

- **Raido has no closures, the example assumes lambdas** — resolved in three passes, each sharper: (1) capture-free lambdas are trivial but only cover constant predicates; (2) filters-as-data (`where()`) covers parameterized filtering and enables indexes; (3) the actual answer is **non-escaping closures**: lambdas may capture anything in scope but are second-class — argument position only, callable or passable, never stored or returned. Captures live on the caller's stack (no arena allocation), can't outlive the call (nothing to serialize — coroutine stacks already serialize), and are semantically equivalent to inlining (no verification hazard; the compiler can inline `filter`/`map` into plain loops, beating function-ref dispatch). Precedents: Swift non-escaping-by-default, Kotlin inline lambdas, Rust borrowed closures. Escaping closures stay cut with a stronger reason than the original: **a closure stored in state pins bytecode inside data**, wrecking the machine-upgrade story — the principle is "code never becomes data." `where()` remains as the indexing path, demoted from ergonomic necessity to optimization.
- **Migration typing is underdesigned.** `old.todos.scan()` needs the previous schema's row type in scope. Synthesized from the old chunk's metadata, hand-redeclared, or imported version-pinned — each has real costs. Own design round before the migration format freezes.
- **Deletion vs. the immutable log.** "Delete my account" meets hash-pinned history — the classic event-sourcing GDPR trap. Compaction helps the main branch; forks pin old history. Likely answer is crypto-shredding (per-subject field encryption, delete the key); v1 needs at least a stated position.
- **Long migrations vs. fuel.** ~~Undesigned~~ — resolved by coroutines (see the Raido section below): a migration yields every N rows, each resume is its own log entry, serialized VM state between chunks. Bounded fuel per chunk, no server stall, exact replay.
- **An "Urd profile" of Raido should be written down**: no coroutines in mutators (atomicity), zero externs beyond the table API, which stdlib modules load. Raido's host-controlled stdlib already precedents this; make it explicit.
- **Fixed-point needs a canonical wire encoding.** 32.32 numbers crossing the JSON boundary must round-trip identically for every client, or hash agreement breaks on the quietest possible detail.
- **Learning curve is the biggest adoption risk, priced consciously.** The programming model is Convex's (transactional mutators + reads) — which proves devs accept the model *in their own language*. Urd asks machine authors to learn Raido. Tiered exposure softens it (client devs never leave TS; agents write machines well — small strict deterministic languages are LLM-friendly), and SQL proves devs learn a language when the payoff is structural. The payoff (replay, fork, verify) must carry the demo, or the language tax wins.
- **Kept after challenge**: the explicit `fx` parameter on every mutator, unused or not. Mild noise, but effects-visible-in-the-signature is Rask's transparency principle applied to Urd; ambient context would be more ergonomic and less honest.

## Does Urd need Raido, or a transaction DSL?

Raised after the store split: what remains VM-side reads like "stored procedures for a deterministic database," and Urd uses none of Raido's headline features — no serializable VM state (ops are fresh short-lived executions), no coroutines, no externs beyond tables. What it uses: determinism, static types, fuel, content-addressing, sandbox. What it misses: lambdas, attributes.

A bespoke transaction DSL is the wrong fix — transaction logic always grows into a general-purpose language (PL/SQL, Solidity), and with Raido itself unbuilt, the real question is which spec to build, not reuse-vs-new. A Core/Script layering of Raido was proposed here, then **withdrawn after a second look**: the "unused" scripting half maps directly onto Urd's own roadmap.

- **Coroutines** solve the open chunked-migration problem outright (a migration is a coroutine yielding every N rows; each resume is a log entry; serialized VM state between chunks lives in the CAS — server never stalls, fuel stays bounded, replay exact) and make **durable workflows** *possible*: `@workflow` coroutines whose frozen state lives in the store and resumes on later ops, deterministically replayable — Temporal's core value inside a branchable database. Possible, not free: wake conditions, event routing, timeouts, and workflow identity are real engine machinery. Migrations-as-coroutines is near-term; `@workflow` is v3+, honestly labeled.
- **Serializable VM state** is the mechanism for both.
- **Externs** were never unused — the `@table` accessors are externs. The pattern extends to a blessed transactional **outbox** (`outbox.emit(...)` writes data; the host acts on it post-commit). Precision matters here: the outbox gives exactly-once *recording* and at-least-once *delivery* — the host can crash between acting and marking acted, so consumers must be idempotent. What stays banned is the game-entity flavor: host functions touching live host state mid-op.

The discipline that survives: plain `@op` mutators never yield — atomic, complete-or-reject. Coroutines enter only as distinct constructs (`@migrate`, `@workflow`) with their own log-entry semantics. Raido stays whole; Urd v1 uses the verification half, Urd v2 reaches into the scripting half. One language, one content-addressed function object shared with Allgard's verifiable transforms.

One more breadcrumb for the merge design round: user-defined merge semantics must be deterministic functions, so **merge handlers will be Raido functions** — the feature we deferred hardest lands on the same VM when it comes.

Side effect on positioning: "a database where transactions are deterministic and history is branchable" is a clearer pitch than "git for live state" — *database* is a category developers already adopt.

## Emergent capabilities

None of these were design goals; each falls out of combining Raido's properties (determinism, serializable state, content-addressed code, fuel, sandbox, coroutines, typed exports). Kept here as the v2+ idea shelf — the pitch stays the v1 pitch.

- **Repro packs** (determinism + fuel): `urd repro <seq>` emits snapshot + ops; any production incident replays to the exact instruction on any machine. "Cannot reproduce" stops existing.
- **Fuel-diff** (determinism + fuel): `urd upgrade --replay` and replay-canary report exact per-op cost deltas — flake-free performance regression gate.
- **Replay-canary** (fork + replay + diff): run yesterday's real ops against a candidate machine, diff hashes and results. Production history as regression suite; the strongest single dev-facing feature on this list.
- **Mobile frozen computation** (serializable coroutines + content addressing): a suspended workflow is bytecode-hash + state — it can move servers mid-flight over Leden. Same primitive as a Midgard NPC crossing domains; the database and the game share one mechanism.
- **Verifiable hosting** (log + machine hash + state hashes): tampering by the host is detectable — certificate transparency for application state. Self-host on untrusted infra.
- **Attestation cache** (machine hash, input hash → output hash): verify once, gossip the result; federated memoization. Allgard's transform verification as an everyday optimization.
- **Type identity by hash** (content-addressed typed exports): schema compatibility is hash equality; clients cache generated bindings by chunk hash. No version negotiation.
- **Database-per-user** (~1KB VM, no ambient authority): thousands of live machines per node; each user gets their own log, branches, workflows. The architecture local-first sync actually wants, at hashmap-entry cost.
- **`urd fuzz`** (DST turned inward): random op sequences against user-declared invariants, counterexamples shrunk by replaying log prefixes. Property-based testing with perfect repro, for every app on Urd.
- **Agent fork-propose-merge**: an AI agent works on a fork of the database; the human diffs and merges. Auditable agent actions with two existing verbs.

## Open questions

- **Rejected ops**: server validates preconditions before append — but is a rejection recorded (auditable, replayable offline queues) or dropped (cleaner log)? Leaning recorded-but-outside-the-chain; needs a design round with the offline story.
- **Migration op ergonomics**: what does writing a state migration actually feel like? Needs a worked example before the format freezes.
- **Snapshot cadence and pruning policy** — mechanical, but affects the shallow-mode trust model.
- **Partial replication** (a client that syncs a subtree): deliberately deferred; noting that Leden object capabilities are the likely shape when it comes.
- **The store determinism contract** (replaces the old state-copy question, which the host-store split dissolved): key ordering for `scan()`, canonical row encoding including None-omission, transactional op semantics — all of it needs its own spec, because every store implementation must match it bit for bit. That includes the client-side store that offline writes (v1.5) require; compiling the Rask store to wasm may beat reimplementing the contract in TS. Biggest open design surface. The contract binds **Urd's own versions** too — the xsnap scar aimed inward: if engine v1.4 encodes or iterates one bit differently than v1.3, every hash breaks with no machine change. Requires a golden test corpus (logs + expected hashes) that every engine release must reproduce, forever.
- **v1 store is deliberately degenerate** — the store split quietly contradicted the "ship sooner" decision by roughly doubling v1's build; the resolution is one ordered map per table, canonical encoding, root hash, no indexes, no `where()`. Weeks, not months; the contract is what the real store slots into later.
- **A branch is a single-threaded write domain.** Server-ordered total order means one core applies a branch's ops sequentially through an interpreter — roughly 1k–100k ops/s per branch depending on op weight. Fine for the target market (TigerBeetle proves single-threaded state machines scale when ops are cheap), but it's a real ceiling and belongs in writing.
