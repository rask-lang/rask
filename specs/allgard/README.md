<!-- id: allgard.overview -->
<!-- status: proposed -->
<!-- summary: Allgard — orchestration of isolated domains (gards) -->

# Allgard

Orchestration layer for gards — isolated domains that communicate through message passing. Uses Leden for transport but is a separate concern.

## What's a Gard

An isolated domain. Own state, own lifecycle. Communicates with other gards only through messages over Leden. No shared memory between gards.

Think of it as: a gard is to Allgard what an actor is to an actor system, what a process is to Erlang's VM, what a service is to a microservice architecture. The isolation boundary.

The difference from actors: gards are coarser-grained. A gard might contain thousands of entities, its own task scheduler, its own pools. It's a world, not an object.

## Why Allgard

Rask's concurrency model (`spawn`, channels, `Shared<T>`) handles parallelism within a single domain well. But structuring a system as multiple isolated domains — where failure in one doesn't crash the others, where domains can be distributed across machines — needs more.

Allgard is that "more." It provides:

1. **Isolation** — gards don't share memory. One gard panicking doesn't corrupt another.
2. **Communication** — typed messages between gards, routed over Leden.
3. **Lifecycle** — start, stop, restart gards. Supervision strategies.
4. **Location transparency** — a gard doesn't know (or care) if the other gard is in-process, on another core, or on another machine.

## Relationship to Other Components

```
┌──────────────────────────────────────────┐
│ Allgard (orchestration)                  │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  │
│  │  Gard A  │  │  Gard B  │  │  Gard C  │  │
│  │ (state,  │  │ (state,  │  │ (state,  │  │
│  │  tasks)  │  │  tasks)  │  │  tasks)  │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  │
│       └──────┬──────┘──────┬──────┘        │
│              │ Leden (transport)            │
└──────────────┴─────────────────────────────┘
```

- **Leden** — moves bytes. No knowledge of gards.
- **Allgard** — manages gards, routes messages through Leden.
- **Raido** — unrelated. Application-specific scripting VM. A gard might host a Raido VM, but that's the application's choice.

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Isolation | No shared memory between gards | Failure isolation. Distribution transparency. |
| Communication | Message passing over Leden | Decoupled from transport. Same API local or remote. |
| Granularity | Coarse — a gard is a domain, not an object | Actors are too fine-grained for game worlds, service boundaries. |
| Supervision | Restart strategies per gard | Erlang got this right. |
| Packaging | Separate crate (`allgard`) | Not every program needs domain orchestration. |

## Open Questions

- **Gard definition syntax.** Declarative? Programmatic? Both?
- **Message routing.** Direct addressing? Topics/channels? Broadcast?
- **Supervision strategies.** One-for-one, one-for-all, rest-for-one? Custom?
- **Hot migration.** Can a gard be serialized and moved to another machine? If so, what are the constraints?
- **Backpressure.** Per-gard mailbox limits? What happens on overflow?
