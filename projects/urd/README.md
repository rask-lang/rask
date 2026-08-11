<!-- id: urd.overview -->
<!-- status: proposed -->
<!-- summary: Urd -- deterministic state-history engine. Git for live application state. -->

# Urd

Independent project. An embeddable state-history engine plus a server binary — the dev-tool face of the stack: [Raido's](../raido/README.md) determinism packaged as something any application can build on.

**Pitch: git for live application state.**

Every serious app rebuilds the same machinery from scratch: offline sync, undo, multiplayer, crash recovery, audit trails, reproducing that one bug. Those aren't seven features. They're one primitive: a deterministic state machine over an append-only op log. Urd is that primitive as a product.

You define the state shape and the operations that change it. Urd owns everything else: durability, ordering, history, branching, replay, verification.

- The log is the truth; state is a cache of it.
- Determinism makes replay exact — any device replays the log and lands on bit-identical state.
- Content addressing gives every state a hash, so histories can be diffed, shared, and verified.

Then the features fall out: sync is shipping the log. Undo is moving a pointer. Branching is forking the log. Multiplayer is agreeing on order. Debugging is replaying to the crash. Audit is the log itself.

## Two artifacts

Like SQLite and Postgres are two shapes of one idea:

1. **Library** — embed it in a Rask program: log store, machine runner, content-addressed snapshots, named branch refs.
2. **Server** — `urd serve app.urd`: hosts the same engine over the network. Clients append ops and subscribe to op streams. One static binary, open protocol, thin clients in any language (TypeScript first).

## Architecture split

The engine is written in Rask. The machines run on Raido.

Same division of labor as Midgard: Rask builds the host — log storage, networking, snapshot store, CLI. Raido runs the logic that must be replayable. Transition code executes on the deterministic VM, so determinism isn't a discipline the user has to maintain — the VM can't be nondeterministic. Effects (time, randomness, external data) enter only as recorded fields inside ops.

## Host API sketch

```rask
import urd

mut db = try urd.open("app.urd")
let main = try db.branch("main")

try main.append(op)                       // apply + log, returns seq
let s = main.state()                      // current state, instant
let old = try db.at(main, seq: 1200)      // time travel
let exp = try db.fork(main, at: 1200, name: "experiment")
main.subscribe(|op, seq| { ... })
```

CLI with git's muscle memory:

```
urd log            urd branch
urd fork main@1200 experiment
urd diff main experiment
urd replay --to 1200
urd verify         # recompute every state hash from the log
```

## The log

Hash-chained entries, git-style: each entry commits to its parent hash and the op; every N ops the engine publishes a state hash. `urd verify` re-runs the log and checks the chain. Anyone holding the log and the machine bytecode can verify a state without trusting whoever produced it — the same property Allgard uses for transforms, applied to application state.

## v1 scope

Deliberately small. A log store, VM hosting, refs, a three-message protocol (hello-with-snapshot, append, op-stream), and the CLI. No piece is research — the research (determinism) is already spent in Raido's design.

- **One branch = one total order.** The server orders the log. Forks exist; merging them back is the app's problem in v1.
- **Server-authoritative writes.** v1 clients get instant local reads, live sync, reconnect-and-catch-up. Queued offline writes come later (they need the VM client-side — Raido compiled to wasm).

## Designed for merge, not shipped with it

**There is no automatic merge in v1, and that's a decision, not a gap.** Concurrent edits on two branches do not converge on their own. Every project that promises automatic convergence spends its entire complexity budget there — Automerge needed a ground-up rewrite to make it viable, ElectricSQL's first system died of the resulting scope. Urd v1 takes total order per branch instead, which is the model production sync engines (Linear-class apps, Replicache/Zero) actually ship.

Merge is still a headline feature — just not day 1, and when it lands it's *rebase*, not CRDT-style convergence: the machine defines what a conflict means, Urd never guesses. The v1 log format carries what merge needs so it can land later without a migration:

- Ops record **intent**, not state diffs.
- Every fork keeps its ancestor point.
- Merge is deterministic rebase: replay the fork's ops onto the target tip. A conflict is an op whose preconditions no longer hold, surfaced to the machine — the machine defines what that means for its domain.

Git punts merging to humans; apps can't. Designing ops so concurrent edits rebase cleanly is the real R&D here, and it deserves its own design round rather than a v1 checkbox.

## Non-goals

- Not a database — no query engine. State is your data structure; expose reads in machine code.
- No consensus, no multi-region. One server orders each log; followers replicate the chain.
- No CRDTs in v1 (see merge above).
- No federation, no permission model in v1 — that's what [Leden](../leden/README.md) capabilities and [Allgard](../allgard/README.md) are for. Urd is deliberately the boring, centralized extraction; the federated story layers on later instead of holding v1 hostage.

## Protocol role

Urd is where the stack meets everyday development. Raido makes verification possible, Leden controls who can do what, Allgard federates sovereign domains — Urd packages the first piece as a tool a developer can adopt in an afternoon for a todo app, and grow into the rest.
