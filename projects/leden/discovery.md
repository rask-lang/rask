<!-- id: leden.discovery -->
<!-- status: proposed -->
<!-- summary: Gossip-based peer discovery and failure detection -->

# Discovery and Health

How Leden endpoints find each other and detect failures. No central registry, no separate infrastructure. Every endpoint participates in gossip — discovery is built into the protocol.

## Why Not a Separate Layer

The original design had Allgard as a separate registry/discovery/health layer on top of Leden. I folded it in. The reasoning:

1. Leden endpoints already have persistent sessions to each other — gossip rides on those for free.
2. Leden already has Introduction — "I know someone you don't, here's a direct capability." That's discovery.
3. A separate registry is a single point of failure. Gossip has no single point of anything.
4. Less infrastructure. The best registry is no registry.

## Peer Gossip

Every endpoint maintains a **peer table** — a list of known endpoints with their addresses and last-seen timestamps.

When two endpoints have a session, they periodically exchange peer table entries. This is protocol-level, not application-level — it's a session maintenance operation, like keepalives.

### What Gets Gossiped

| Field | Type | Purpose |
|-------|------|---------|
| `endpoint_id` | cryptographic identity | Who |
| `addresses` | list of transport addresses | Where (may have multiple — TCP, QUIC, etc.) |
| `last_seen` | timestamp | When someone last heard from them |
| `metadata` | optional key-value | Application-defined tags ("region=eu", "role=worker") |
| `generation` | monotonic counter | Increments on restart. Distinguishes "same endpoint, new process" from stale data. |

Gossip is pull-based. Endpoints exchange digests (compact summaries of what they know), then request full entries for anything they're missing or that's newer. This bounds gossip bandwidth — you don't send the full table every time.

### Protocol

| Message | Direction | Purpose |
|---------|-----------|---------|
| `PeerDigest` | Bidirectional | "Here's a summary of who I know and when I last heard from them" |
| `PeerRequest` | Bidirectional | "Send me full entries for these endpoint IDs" |
| `PeerUpdate` | Bidirectional | "Here are the entries you asked for" |

Gossip messages are exchanged on existing sessions. No special connections needed.

### Convergence

Gossip converges in O(log N) rounds for N endpoints — each round, each endpoint tells a few peers, who tell a few more. A new endpoint joining a 1000-node network is known everywhere within seconds, not minutes.

The tradeoff: gossip is eventually consistent. Two endpoints might briefly disagree about who exists. For discovery, this is fine — you learn about peers as fast as information can spread, and a few seconds of staleness at the edges doesn't matter.

## Joining the Network

A new endpoint needs exactly one thing: the address of any existing endpoint (a **seed**).

### Join Flow

```
New endpoint                    Seed endpoint
    |                                |
    |  (transport connect)          |
    |  Hello(...)                   |
    |───────────────────────────────>|
    |                                |
    |  Welcome(...)                 |
    |<───────────────────────────────|
    |                                |
    |  Bootstrap → greeter          |
    |───────────────────────────────>|
    |                                |
    |  (greeter returns caps)       |
    |<───────────────────────────────|
    |                                |
    |  PeerDigest (empty)           |
    |───────────────────────────────>|
    |                                |
    |  PeerUpdate (seed's table)    |
    |<───────────────────────────────|
    |                                |
    |  (now knows about other       |
    |   endpoints, can connect      |
    |   to them directly)           |
```

The new endpoint connects to the seed, bootstraps normally, then receives the seed's peer table through gossip. From there it can connect to other endpoints directly using Introduction or by initiating its own sessions.

Multiple seeds can be configured for redundancy. If the first is down, try the next. Once you're in the network, you don't need seeds anymore — you learn about new peers through gossip.

### Seed Configuration

Seeds are transport addresses, not capabilities. They're the equivalent of DNS root servers — well-known entry points. A deployment might have 2-3 stable seeds.

Seeds aren't special. Any endpoint can be a seed. The only requirement is that seeds should be long-lived and reachable — ideally the most stable endpoints in the network.

## Failure Detection

Gossip-based, inspired by the SWIM protocol. Distributed, no single monitor.

### How It Works

1. Every endpoint periodically pings a random peer (direct probe).
2. If the peer doesn't respond within a timeout, the endpoint asks K other peers to try (indirect probe).
3. If indirect probes also fail, the endpoint marks the peer as **suspect**.
4. Suspect status is gossiped. Other endpoints confirm or deny based on their own observations.
5. If enough time passes without anyone hearing from the suspect, it's marked **down**.
6. Down status is gossiped. Endpoints that care (had sessions to the downed endpoint) handle it.

### States

| State | Meaning |
|-------|---------|
| `Alive` | Responding to probes. Normal. |
| `Suspect` | Failed a probe. Might be a network blip or genuinely down. |
| `Down` | Multiple probes failed across multiple observers. Consensus: it's gone. |

### Integration with Sessions

Leden sessions already detect transport failures (Layer 1 reconnection). Discovery's failure detection is complementary:

- **Session-level**: detects failure of a specific connection between two endpoints. Triggers reconnection.
- **Discovery-level**: detects failure of an endpoint as seen by the network. Triggers notification to all interested parties.

A session might reconnect (transport blip) while discovery still considers the endpoint alive. Or discovery might mark an endpoint as down while its sessions are still trying to reconnect. Both are correct — they're observing different things.

### Notification

When discovery marks an endpoint as down, it doesn't automatically tear down sessions or revoke capabilities. It notifies the application: "endpoint X appears to be down." The application decides what to do — wait, failover, notify users, whatever.

This is deliberate. Discovery detects. The application reacts. Leden doesn't make policy decisions about what "down" means for your use case.

## What This Doesn't Do

- **Topology management.** Discovery tells you who exists. It doesn't tell you who should connect to whom. That's the application's decision. A game server connects to its neighboring regions. A build system connects to the coordinator. Discovery provides the addresses; the application provides the logic.
- **Process supervision.** An endpoint is down. Discovery tells you. Restarting it is your deployment platform's job. Leden doesn't manage processes.
- **Load balancing.** Discovery doesn't route traffic or balance load. If you need that, build it on top using the peer metadata ("this endpoint is at 80% CPU").

## Discovery Is an Extension

Like observation and content store, discovery is negotiated during the version handshake. Not every endpoint needs it — two processes on the same machine connected via Unix socket don't need peer gossip.

```
Hello(min=1, max=1, ext=[discovery])
Welcome(version=1, ext=[discovery])
```

If an endpoint doesn't support discovery, it simply doesn't participate in gossip. It can still be connected to directly if you know its address. It just won't be found automatically.

## Resolved

**Metadata schema.** No standardized keys. All metadata is application-defined string key-value pairs. Standardizing "region" or "role" assumes we know what applications need — we don't. Applications that care about a common vocabulary can publish their own conventions. The protocol just carries the bytes.

The only protocol-level metadata is already in the peer table entry: `endpoint_id`, `addresses`, `last_seen`, `generation`. That's enough for discovery. Everything else is application concern.

**NAT traversal.** Relay through a public endpoint. Leden already has sessions and introductions — a NAT'd endpoint establishes an outbound session to a public endpoint, which relays messages on its behalf. This composes from existing mechanics: the public endpoint introduces the NAT'd endpoint to others, who then communicate through the relay session.

Discovery must not gossip unreachable addresses. A NAT'd endpoint advertises its relay's address plus a session identifier, not its private IP. The relay adds one hop of latency. For endpoints that need direct connections, standard hole-punching (coordinated through the relay) can establish a direct transport — but this is a transport-layer optimization, not a protocol requirement.

STUN/TURN/ICE are heavy. A Leden relay is lighter because it already speaks the protocol — no translation layer.

**Relay security.** A relay sees all traffic between the NAT'd endpoint and its peers. Without protection, a compromised or malicious relay can read, modify, or drop messages. Two requirements:

1. **TLS required on relay sessions.** The session between the NAT'd endpoint and the relay must use TLS (or equivalent transport encryption). This isn't the general "TLS is optional" rule from the transport layer — relay sessions are a special case because the relay is a privileged intermediary. In-process and localhost sessions don't need TLS because there's no intermediary. Relay sessions always have one.

2. **Capability attenuation on relay grants.** The relay receives an attenuated capability for message forwarding — specifically `relay` permission only, not `invoke` or `observe`. The relay can route messages to/from the NAT'd endpoint but cannot call methods on the endpoint's objects, observe its state, or delegate capabilities on its behalf. The relay is a dumb pipe with a narrow permission scope.

**What this doesn't cover:** End-to-end encryption between the NAT'd endpoint and its peers (through the relay). TLS protects NAT'd↔relay and relay↔peer separately, but the relay can still observe cleartext at the application layer. True end-to-end encryption through relays would require encrypting the message payload before it enters the Leden message layer — that's a transport-layer extension, not a protocol requirement. For most deployments, TLS on both hops is sufficient. For high-security deployments, an E2E encryption extension can be negotiated during the handshake.

## Deferred

- **Gossip protocol tuning.** Fanout, probe interval, suspect timeout, down threshold. All configurable per deployment. Sensible defaults will come from implementation experience, not spec work. Target: 10–1000 endpoints for defaults.
- **Large networks.** Hierarchical gossip (intra-zone full gossip, inter-zone summaries) is the known solution. Not designing it now — YAGNI until someone actually runs 10,000+ endpoints.
