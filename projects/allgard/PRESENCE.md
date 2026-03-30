<!-- id: allgard.presence -->
<!-- status: proposed -->
<!-- summary: Owner presence — location, observability, reachability, standard relationship grants -->

# Presence

Where Owners are, and how to reach them.

The [Conservation Laws](CONSERVATION.md) govern how Objects behave. This spec governs how Owners are located, observed, and contacted across domains. Presence is the observable consequence of Owners operating in a distributed federation — if entities exist across domains, their location is state, and state should be formalized.

## Why This Is Physics

The Conservation Laws define invariants: supply is conserved, ownership is singular, exchange balances. These are the physics of *things*.

Presence is the physics of *entities*. An Owner has a location — the Domain(s) where it currently operates. That location is observable, changes over time, and affects what interactions are possible. A domain that hosts an Owner's leased objects needs to know the Owner is still there. A trading partner needs to know the Owner is reachable. An automated system needs to know its counterpart is online.

This isn't a social feature. It's the same kind of fact as "this Object is on Domain X" — which is already an Object property in [PRIMITIVES.md](PRIMITIVES.md). Presence extends that to Owners.

## Owner Location

An Owner's **presence** is the set of Domains where the Owner currently has active authenticated sessions.

Presence is derived from Leden session state. When an Owner opens an authenticated session with a Domain, that Domain is in the Owner's presence set. When the session closes, it's removed. No separate presence state to maintain — it falls out of existing session management.

### Properties

| Property | Description |
|----------|------------|
| `home_domain` | The Owner's authoritative domain (already defined in [PRIMITIVES.md](PRIMITIVES.md#home-domain)) |
| `active_domains` | Set of Domains where the Owner has current authenticated sessions |

### Not Singular

Objects are on exactly one Domain (Law 2). Owners are not. An Owner visiting Domain B while maintaining a session with home Domain A is present on both. An automated Owner might have sessions with dozens of Domains simultaneously. There's no conservation of presence — it's not a scarce resource, it's observable state.

I considered making presence singular ("an Owner is on one Domain at a time") and rejected it. The leased transfer model already has the Owner connected to both home and visited domain. Forcing singularity would either break that model or require pretending one of the sessions isn't real.

### Offline

An Owner with no active sessions has an empty presence set. This is valid state — not every Owner is always online. The home domain may still hold the Owner's objects and accept inbound Transforms on their behalf, but the Owner isn't actively operating.

## Observability

Presence and profile are observable state. Other Owners can subscribe to changes via [Leden observation](../leden/observation.md), gated by a Grant.

### How It Works

1. Owner A grants Owner B a `presence` Grant (see [Standard Grants](#standard-grants) below).
2. Owner B subscribes to Owner A's presence via Leden observation at Owner A's home domain.
3. When Owner A's presence changes (connects to a new domain, disconnects from one), Owner B receives an update.
4. Owner B can also observe Owner A's [profile Object](PRIMITIVES.md#profile) at the same home domain — avatar, bio, and any other profile fields update automatically.

The home domain is the canonical observation point. It knows the Owner's session state because it manages leases and tracks where the Owner's objects are. Observing presence at the home domain gives a complete view. Observing at a visited domain only tells you about that specific domain.

### Capability Gating

Presence is not observable by default. You need a Grant. This is different from Object observation, where holding a reference implies observation permission. The reason: Objects are things; Owners are entities. Knowing where an entity is located is a stronger capability than knowing what a thing contains.

A Domain's greeter does NOT grant presence observation on its hosted Owners. A stranger connecting to a domain can see what the domain hosts (catalog observation), but not which specific Owners are currently active. That would make every greeter a tracking service.

### What Updates Contain

A presence update is a set change:

```
PresenceUpdate(owner_id, added: [domain_b], removed: [])
PresenceUpdate(owner_id, added: [], removed: [domain_b])
```

The observer receives which domains were added to or removed from the Owner's active set. Snapshot-on-subscribe provides the initial full set. Delta updates follow. Sequence numbers and backpressure follow Leden observation conventions.

## Reachability

Knowing where an Owner is located enables contacting them. Reachability is the practical consequence of presence.

### Routing Through Home Domain

The home domain is the canonical contact point for an Owner. To reach Owner A:

1. Contact Owner A's home domain (the home domain identity is part of the Owner's public identity, or discoverable via gossip).
2. If Owner A is on the home domain: the home domain delivers the message directly.
3. If Owner A is visiting Domain B: the home domain knows this (it manages the lease) and can either forward or provide Domain B's endpoint so the caller can contact directly.

This is the same routing model as email (MX records point to the authoritative server) without the centralization (any Owner can change their home domain).

### Direct Contact

If the caller already knows which domain the Owner is on (from a presence subscription or prior interaction), they can contact that domain directly. No routing through home. The visited domain validates the caller's Grant and delivers the message.

Direct contact is an optimization, not a requirement. Home routing always works as a fallback.

### Unreachable Owners

If an Owner is offline (empty presence set), messages can be:

1. **Queued at the home domain.** The home domain holds messages until the Owner connects. Domain policy determines queue duration and size limits.
2. **Rejected.** The caller receives "Owner unreachable." No queuing obligation.

The home domain's choice. Some domains queue indefinitely. Some reject after 24 hours. Some don't queue at all. This is domain policy, not protocol.

## Standard Grants

The [Grant](PRIMITIVES.md#grant) primitive is general — any scope, any target, any expiry. Standard Grants are named conventions for common relationship patterns. They don't add new capabilities — they're pre-defined scope configurations that domains recognize consistently across the federation.

Without these, every domain invents its own "friend" Grant with different semantics, and cross-domain relationships don't interoperate.

### `presence`

Unidirectional presence visibility. The grantee can observe which Domains the grantor is currently active on.

| Property | Value |
|----------|-------|
| `scope` | `observe_presence` |
| `target` | The grantor's Owner identity |
| `revocable` | Yes (always) |

This is the minimal social relationship. "You can see where I am." No messaging, no interaction beyond observation.

### `contact`

Presence observation plus direct messaging. The grantee can observe the grantor's presence AND send messages to the grantor, routed through the home domain or directly.

| Property | Value |
|----------|-------|
| `scope` | `observe_presence` + `message` |
| `target` | The grantor's Owner identity |
| `revocable` | Yes (always) |

The `message` scope means the grantee can submit messages to the grantor's home domain for delivery. The home domain enforces rate limits (Law 5 applies to messages like any other operation).

### `block`

Revocation of all inbound Grants from a specific Owner, plus an advisory suppression signal.

Blocking is not a Grant — it's a **revocation pattern**:

1. Revoke every Grant where the blocked Owner is the grantee.
2. Instruct the home domain to reject future Grant offers from the blocked Owner.
3. Optionally, signal to visited domains: "reject interactions from this Owner on my behalf." This is advisory — visited domains decide whether to honor it.

Step 3 is advisory because sovereignty means a domain decides its own interaction policy. A visited domain may choose to honor blocks from visiting Owners, or not. The home domain always honors them (it's authoritative for the Owner's policy).

### `group`

Multi-party mutual `contact` Grants with a designated administrator.

A group is not a primitive. It's a pattern composed from Grants:

1. An administrator Owner creates a group identity (an Object of type `group`).
2. The administrator issues `contact` Grants to each member, scoped to the group.
3. Members receive mutual `contact` Grants — they can observe presence and message each other.
4. The administrator can add/remove members by issuing/revoking Grants.

Groups compose from existing primitives (Object + Grant). The "standard" part is the type tag (`group`) and the expected Grant structure, so any domain recognizing the `group` type can display membership, route group messages, and honor administrator actions.

### Cross-Domain Consistency

Standard Grants carry their type tag (`presence`, `contact`, `group`) so receiving domains know the convention. A `contact` Grant issued on Domain A is recognized on Domain B with the same semantics — Domain B knows what `observe_presence` + `message` means because the convention is standardized.

Without this, a Grant scoped `[0x42, 0x07]` on Domain A means nothing to Domain B. Named conventions give Grants portable semantics.

## What This Doesn't Cover

- **Spatial location within a domain.** Presence says "Owner is on Domain X." It doesn't say "Owner is at coordinates (3,4) in region Y." Spatial protocols build on top of presence.
- **Profile content schema.** The Owner's [profile Object](PRIMITIVES.md#profile) is defined as a mechanism in Allgard. The content schema (which fields, what they mean) is defined in [GDL](../gdl/) — content description is GDL's job.
- **Voice or media channels.** Real-time media between Owners is an application feature using Leden transport. Presence tells you who's reachable; media setup is a separate capability negotiation.
- **Notification preferences.** Whether an Owner wants to be disturbed, what channels they prefer — local policy stored at the home domain, not a federation concern.

## Relationship to Existing Specs

| Spec | Relationship |
|------|-------------|
| [PRIMITIVES.md](PRIMITIVES.md) | Extends the Owner primitive with presence state. Uses the Grant primitive for standard conventions. |
| [CONSERVATION.md](CONSERVATION.md) | No changes. Presence is not a conservation law. Law 5 (bounded rates) applies to presence updates and messaging like any other operation. |
| [TRANSFER.md](TRANSFER.md) | Leased transfer already implies Owner presence on visited domains. Presence formalizes what was implicit. |
| [TRUST.md](TRUST.md) | Presence enables richer trust signals — Owners that are consistently reachable build reputation faster than ghosts. |
| [Leden observation](../leden/observation.md) | Presence updates are delivered via Leden observation. All existing observation semantics (backpressure, reconnection, filtered observation) apply. |

## Open Questions

- **Presence privacy across trust levels.** Should a `presence` Grant reveal which specific Domains the Owner is on, or just "online/offline"? Full domain list is useful for routing but reveals movement patterns. Maybe two Grant scopes: `observe_liveness` (online/offline only) and `observe_presence` (full domain set). Keeping it simple for now — one scope, full presence. Can split later if privacy concerns are real.
- **Home domain migration.** An Owner can change their home domain. What happens to existing presence subscriptions? The old home should redirect observers to the new home. This is a protocol concern that needs specifying — probably in TRANSFER.md, since it's a special case of identity transfer.
