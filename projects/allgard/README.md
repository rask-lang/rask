<!-- id: allgard.overview -->
<!-- status: superseded -->
<!-- summary: Allgard has been folded into Leden -->

# Allgard — Superseded

Allgard's responsibilities have been folded into [Leden](../leden/).

## What Happened

Allgard was a separate registry, discovery, topology, and health layer for networks of gards. I decided it was unnecessary overhead — Leden already has persistent sessions, Introduction (third-party handoff), and the extension mechanism needed to support discovery natively.

The replacement is gossip-based discovery built into Leden as a protocol extension. See [leden/discovery.md](../leden/discovery.md).

## What Moved Where

| Was Allgard | Now |
|-------------|-----|
| Registry | Gossip peer tables — every endpoint shares who it knows |
| Discovery | Join any seed endpoint, learn the rest through gossip |
| Health | SWIM-inspired failure detection — cooperative, no central monitor |
| Topology | Dropped. Applications decide who connects to whom. |

## Why

1. Leden sessions already exist between endpoints — gossip rides on them for free.
2. Introduction already lets A introduce B to C directly. That's discovery.
3. A central registry is a single point of failure. Gossip has none.
4. One less crate, one less layer, one less thing to deploy.

## The Term "Gard"

The term "gard" (an isolated server process with its own state) is still useful. It just doesn't need a dedicated orchestration layer. A gard is a Leden endpoint that participates in gossip — nothing more.
