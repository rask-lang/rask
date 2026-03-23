<!-- id: allgard.overview -->
<!-- status: proposed -->
<!-- summary: Allgard — topology, registry, and discovery for networks of gards -->

# Allgard

The map of the multiverse. Allgard knows what gards exist, where they are, and how to find them. [Leden](../leden/) is the web between them.

## What's a Gard

A server process with isolated state. Runs independently, talks to other gards over Leden. Could be a game world region, a microservice, a build worker — whatever. Allgard doesn't care what's inside.

Gards are coarse-grained. A gard might contain thousands of entities, its own task scheduler, its own pools. It's a domain, not an object.

## What Allgard Does

Leden gives you point-to-point capability-based communication. But it doesn't answer: what endpoints exist? Where are they? How does a new gard join? How do I know one went down?

Allgard fills that gap:

1. **Registry** — what gards exist and where they are. The directory.
2. **Topology** — how gards relate. Which ones should know about each other at startup. What capabilities they bootstrap with.
3. **Discovery** — a new gard joins the network and finds the others. An existing gard moves and the others find its new address.
4. **Health** — is a gard up? Leden detects connection drops. Allgard decides what that means — report it, alert an operator, notify dependent gards.

## What Allgard Does Not Do

- **Communication** — that's Leden. Allgard helps you find the address; Leden handles the session, capabilities, and messages.
- **Process supervision** — restarting crashed processes is your deployment platform's job (systemd, Docker, Kubernetes). Allgard reports health, it doesn't manage processes.
- **Application logic** — Allgard doesn't know about game worlds, conservation laws, or Raido VMs. That's Midgard.

## How It Fits Together

```
┌─────────────────────────────────────────────────┐
│ Allgard (registry, topology, discovery, health) │
│                                                 │
│   "Gard A is at 10.0.1.5:9000"                │
│   "Gard B is at 10.0.2.3:9000"                │
│   "Gard C just joined at 10.0.3.1:9000"       │
│   "Gard A hasn't responded in 30s"             │
│                                                 │
├─────────────────────────────────────────────────┤
│ Leden (protocol, sessions, capabilities)        │
│                                                 │
│   Gard A ←──session──→ Gard B                  │
│   Gard B ←──session──→ Gard C                  │
│                                                 │
├─────────────────────────────────────────────────┤
│ Transport (TCP, QUIC, Unix socket)              │
└─────────────────────────────────────────────────┘
```

Allgard is DNS + service mesh. Leden is the protocol on the wire.

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Scope | Registry, topology, discovery, health | Everything between "I have a protocol" and "I have a running system." |
| Not process management | Delegate to OS/infra | Reinventing systemd is not the goal. |
| Packaging | Separate crate (`allgard`) | Many Leden users won't need managed topology. |
| Granularity | Coarse — gards are servers, not objects | Fine-grained actors don't need a registry. Server processes do. |

## Open Questions

- **Registry implementation.** Central registry gard? Gossip protocol? Static config file? Probably all three for different scales.
- **Topology definition.** Declarative manifest? Programmatic API? How do you describe "these 5 gards form a cluster, this one is standalone"?
- **Health semantics.** What's the contract? Heartbeat interval? What does "unhealthy" mean — just report it, or trigger topology changes?
- **Dynamic membership.** Gards joining and leaving at runtime. How does the registry propagate changes? Eventually consistent is fine, but what's the consistency window?
- **Migration.** Can a gard's address change (move to a different machine) without breaking existing Leden sessions? Sturdy references help — reconnect and re-present credentials. But the registry needs to know the new address.
