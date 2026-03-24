<!-- id: leden.observation -->
<!-- status: proposed -->
<!-- summary: Push-based observation of object state changes -->

# Observation

How Leden handles "notify me when this changes." Without this, every real-time system built on Leden is polling. Polling over a network is wrong — you have a persistent session, use it.

## The Problem

The protocol has request/response (method calls) and chained operations (promise pipelining). Both are pull-based: the caller asks, the responder answers. But most real-time systems need push:

- Game state changes → update all observers
- Inventory changes → update the UI
- Health monitoring → alert when something drops
- Collaborative editing → sync between participants

Polling is the obvious workaround, and it's bad in every way: wastes bandwidth, adds latency (you see changes on the next poll, not when they happen), and scales poorly (N observers × M objects × poll frequency = too many messages).

## Design

Observation is a protocol-level operation, not something each object implements ad hoc. This is important — backpressure, reconnection, and capability gating are protocol concerns.

### Core Operations

| Operation | Direction | Purpose |
|-----------|-----------|---------|
| `Observe(object_ref)` | Client → Server | "Notify me of changes to this object" |
| `Update(observation_id, payload)` | Server → Client | "This object changed, here's what happened" |
| `Unobserve(observation_id)` | Client → Server | "Stop notifying me" |

That's it. Three operations. Everything else is policy on top.

### Capability Gating

You can only observe objects you have a capability for. Observation isn't a separate permission by default — if you can read an object, you can observe it. But capabilities can be attenuated to exclude observation:

```
Full capability:     read + write + observe
Attenuated:          read + write              (no push updates)
Further attenuated:  read                      (no write, no push)
```

This matters for load control. A high-traffic object might have thousands of readers but only dozens of observers. The object's host can issue read-only capabilities to most clients and observe-capable ones to the few that need real-time updates.

### What Gets Sent

The server decides what an update contains. The protocol defines the envelope (observation_id + payload), not the contents. Two strategies:

**Deltas** — "field X changed from A to B." Small. Efficient. Requires the observer to maintain state and apply deltas in order.

**Snapshots** — "here's the entire current state." Larger. Simpler. The observer doesn't need to track history.

The right choice depends on the object. A game entity with 20 fields where one changes per tick → delta. An observer that just reconnected and missed 500 updates → snapshot. Most objects will use deltas for normal operation and snapshots for catch-up.

The protocol carries a **sequence number** on every update. This lets the observer detect gaps (missed updates during a network blip) and request a snapshot to resync.

```
Update(id=7, seq=142, delta: {health: 50 → 43})
Update(id=7, seq=143, delta: {position: (3,7) → (4,7)})
   ... network blip, seq 144-148 lost ...
Update(id=7, seq=149, delta: {health: 43 → 30})
   observer detects gap, requests resync
Update(id=7, seq=149, snapshot: {health: 30, position: (6,8), ...})
```

### Backpressure

A fast-changing object shouldn't overwhelm a slow observer. Credit-based flow control:

1. Observer grants N credits when subscribing (e.g., 10).
2. Each update consumes one credit.
3. When credits hit zero, the server stops sending updates to that observer.
4. Observer grants more credits when it's ready (e.g., after processing a batch).

When backpressured, the server has options:

| Strategy | Behavior | When |
|----------|----------|------|
| **Buffer** | Queue updates, send when credits arrive | Short bursts, observer will catch up |
| **Coalesce** | Merge pending updates into one snapshot | Observer is slow, intermediate states don't matter |
| **Drop** | Discard old updates, send latest | Real-time systems where stale data is worse than gaps |

The object decides the strategy. A stock ticker drops stale prices. A chat log buffers. A game entity coalesces — you want the latest position, not every position along the path.

### Multiplexing

One session, many observations. Each observation is a logical stream within the session's multiplexing (Layer 1). No per-observation connection overhead.

An observer watching 200 objects in a game region has 200 observations over one session. The session's backpressure applies globally (the transport can only carry so much), and per-observation credits apply locally (a chatty object doesn't starve a quiet one).

### Session Reconnection

When a session drops and reconnects:

1. The server remembers active observations (tied to the session's sturdy reference).
2. On reconnect, observations resume automatically.
3. The server sends a snapshot for each observation to resync (the observer may have missed updates during the gap).
4. Normal delta flow continues from the snapshot's sequence number.

The observer doesn't re-subscribe manually. This falls out of Leden's session-capability decoupling — the session reconnects, capabilities re-attach, observations resume.

If the observer was down long enough that the server evicted its observation state, the observer gets an `ObservationExpired` error and must re-subscribe. This is the server's eviction policy, not a protocol rule.

### Filtered Observation

Not every observer wants every field. A minimap UI cares about position, not health. A damage log cares about health changes, not position.

Filters are specified at subscribe time:

```
Observe(object_ref, filter: [position, velocity])
```

The server only sends updates matching the filter. This reduces bandwidth and processing on both sides. Filters are optional — omit for "everything."

Filters can be updated on a live observation without unsubscribing and resubscribing. The server applies the new filter immediately.

### Fan-Out

One object, many observers. The server is responsible for fan-out — sending updates to all observers of an object. This is a server-side scaling concern, not a protocol concern.

The protocol intentionally doesn't specify how the server manages fan-out internally. It could be a simple loop, a pub/sub bus, or a dedicated broadcast tree. The protocol only cares about the per-observer stream.

However: fan-out is where observation gets expensive. An object with 10,000 observers sending 60 updates/second is 600,000 messages/second. The protocol provides the tools to manage this (backpressure, coalescing, filtering), but the server must be smart about it. This is an implementation concern, not a spec problem.

## Observation Is an Extension

Observation is negotiated during the version handshake as an extension (see protocol.md, Version Negotiation). Not every endpoint needs it. A build system using Leden for RPC doesn't need push updates. A game server does.

```
Hello(min=1, max=1, ext=[observation])
Welcome(version=1, ext=[observation])
```

If the server doesn't support observation, `Observe` calls return `MethodNotFound`. The client falls back to polling or decides it can't operate.

## What This Doesn't Cover

- **Derived observations.** "Notify me when any object in this collection changes." That's application logic — the application observes the collection and decides what to forward.
- **Cross-endpoint observation relay.** Observer on A wants to watch an object on B, routed through C. The protocol handles this through capability delegation — C introduces A to B, A observes directly. No relay.
- **Observation persistence.** Should observations survive endpoint restarts? Currently: no. Observations are session-scoped. Sturdy references let you re-establish them, but they don't auto-resume across restarts. This might be wrong for some use cases — open to revisiting.

## Resolved

**Observation on promises.** Yes. `Observe` accepts a promise reference. The server queues the observation and activates it when the promise resolves. If the promise rejects, the observation returns the same error. The gap between resolution and first update is handled by the existing snapshot-on-subscribe behavior — the observer gets a snapshot as soon as the observation activates.

This composes naturally with promise pipelining: "fetch the region, then observe it" is one round trip.

**Observation groups.** Yes. `ObserveBatch(refs[])` subscribes to many objects in one message. Returns per-item results — same pattern as `Reattach`. Partial failure is per-item: 197 succeed, 3 return `ObjectNotFound`, the observer handles each independently. No all-or-nothing semantics.

`UnobserveBatch(ids[])` for the matching teardown.

**Server-initiated observation.** No. The capability model is clear: the client decides what it observes. Holding a reference is permission; exercising it is a choice.

The server-push pattern ("you're in this region, here are the objects") is handled differently: the server sends the client a batch of object references. The client subscribes to the ones it cares about. Two messages instead of one, but the authority model stays clean. The server suggests; the client decides.
