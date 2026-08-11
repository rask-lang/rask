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

## Design implications

Decisions the survey settles:

1. **Log entry** = `{parent_hash, machine_hash, op {name, version, args, recorded_effects}, server_time, seq}`, with a state hash published every N entries. Hash-chained; `verify` re-runs and checks.
2. **Ops are named mutators with typed args** (intent, not diffs). Effects — time, randomness, external data — are recorded fields in the op, stamped by the server at append. Nothing else crosses into the VM.
3. **Machine versioning via content addressing.** Bytecode lives in the CAS; entries pin their machine hash; replay uses the pinned version; upgrades are explicit migration ops. This is the anti-Temporal design and it falls out of infrastructure we already need.
4. **Reads are machine functions** run against current state, ABCI-Query style: allowed to be nondeterministic in formatting, never logged, never part of agreement. Plus whole-state export for small apps and debugging.
5. **Storage engine is boring**: append-only segment files + CAS + refs, git's loose/packed lifecycle. No custom B-tree, no embedded SQL engine.
6. **Compaction and shallow mode from v1**, because history growth killed or nearly killed everyone who ignored it.
7. **Desync tooling is a feature**: periodic state hashes, and a report that binary-searches the log for the first divergent entry.
8. **DST harness for the engine before features.**
9. **Offline writes (v1.5) reuse the Replicache rebase model** — speculative ops client-side, rebase on server order — with one implementation instead of two, once Raido runs in wasm.

## Open questions

- **Rejected ops**: server validates preconditions before append — but is a rejection recorded (auditable, replayable offline queues) or dropped (cleaner log)? Leaning recorded-but-outside-the-chain; needs a design round with the offline story.
- **Migration op ergonomics**: what does writing a state migration actually feel like? Needs a worked example before the format freezes.
- **Snapshot cadence and pruning policy** — mechanical, but affects the shallow-mode trust model.
- **Partial replication** (a client that syncs a subtree): deliberately deferred; noting that Leden object capabilities are the likely shape when it comes.
