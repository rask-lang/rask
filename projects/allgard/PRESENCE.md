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

Presence operates at two tiers. The distinction matters: local presence is physics (you see who's in the room), cross-domain presence is a relationship (you choose who can track you).

### Tier 1: Local Presence (Same Domain)

When you're on a Domain, you can see who else is there. No Grant needed.

This falls out of existing mechanics. A Domain sends you its region state via [GDL](../gdl/GDL.md) — entities in the region, their names, their properties. Some entities are Owners (kind `agent`). Seeing them is just observing the region. The Domain already decides what entities you see (viewport filtering, fog of war, stealth mechanics). Local presence is part of that — not a separate system.

**The Domain controls local visibility, not the Owner.** If the Domain shows you who's in the tavern, that's the Domain's policy. An Owner can't opt out of being visible to co-located Owners — that would be like demanding invisibility in someone else's house. What the Owner controls is which Domains they visit. What the Domain controls is what visitors see.

| Domain policy | What visitors see | Use case |
|---|---|---|
| Full identity | Name, kind, profile ref | Social spaces, taverns, marketplaces |
| Masked identity | "A hooded traveler" | Anonymity-supporting domains |
| Selective visibility | Some Owners hidden | Stealth mechanics, admin-invisible mode |
| No visitor list | Only entities you directly interact with | Privacy-focused domains |

This is sovereignty. The protocol doesn't prescribe which policy a Domain uses. It requires that co-located Owners are representable as GDL entities — the Domain decides which Owners become visible entities and how much identity information to expose.

**What local presence reveals:**

At minimum, the Owner's GDL entity representation — whatever the Domain chooses to expose. At maximum, the entity carries a reference to the Owner's identity, letting the observer resolve name, kind, and profile. The Domain decides where on this spectrum each Owner lands.

**What local presence does NOT reveal:**

Which *other* Domains the Owner is connected to. Local presence tells you "this Owner is here." It doesn't tell you "this Owner is also on Domain C and Domain D." That's cross-domain presence — Tier 2.

### Tier 2: Cross-Domain Presence (Granted)

Seeing where an Owner is when you're NOT in the same place. This requires a `presence` or `contact` Grant.

Cross-domain presence is the relationship layer. It answers: "where is my friend right now?" This is the tracking-capable tier — knowing someone's location across the federation — which is why it requires explicit authorization.

#### How It Works

1. Owner A grants Owner B a `presence` Grant (see [Standard Grants](#standard-grants) below).
2. Owner B subscribes to Owner A's presence via Leden observation at Owner A's home domain.
3. When Owner A's presence changes (connects to a new domain, disconnects from one), Owner B receives an update.

The home domain is the canonical observation point. It knows the Owner's session state because it manages leases and tracks where the Owner's objects are. Observing presence at the home domain gives a complete view. Observing at a visited domain only tells you about that specific domain.

### Presence vs Profile Observation

Presence and the [profile Object](PRIMITIVES.md#profile) are **separate observations**. A `presence` or `contact` Grant authorizes both, but they're independent Leden observation subscriptions:

| Observation | What changes | Frequency | Subscribe to |
|---|---|---|---|
| Presence | Domain set (connect/disconnect) | Minutes to hours | Owner's presence state at home domain |
| Profile | Avatar, bio, display name, extensions | Rarely (days, weeks) | Owner's profile Object at home domain |

Separating them matters for bandwidth. An Owner tracking 200 contacts doesn't need profile updates every time someone connects to a new domain. Conversely, a domain rendering a player's profile card doesn't need real-time domain-set changes. Clients subscribe to what they need.

Both use standard Leden observation semantics — snapshot on subscribe, delta updates, sequence numbers, backpressure. Profile observation uses Leden's [filtered observation](../leden/observation.md#filtered-observation) if the client only cares about specific fields (e.g., avatar only).

### What Updates Contain

A presence update is a set change:

```
PresenceUpdate(owner_id, added: [domain_b], removed: [])
PresenceUpdate(owner_id, added: [], removed: [domain_b])
```

The observer receives which domains were added to or removed from the Owner's active set. Snapshot-on-subscribe provides the initial full set. Delta updates follow. Sequence numbers and backpressure follow Leden observation conventions.

## Reachability

Knowing where an Owner is located enables contacting them. Reachability is the practical consequence of presence.

### Name Resolution

An Owner's global address is `name@home_domain` (see [PRIMITIVES.md](PRIMITIVES.md#name)). To resolve a name to a contactable identity:

1. **Resolve the domain.** Look up `home_domain` via Leden gossip/discovery. This gives you a transport address — same as DNS resolving a hostname to an IP.
2. **Contact the greeter.** Hit the home domain's greeter. The greeter is the public entry point for every domain (see [Bootstrapping](README.md#bootstrapping)).
3. **Resolve the name.** Send a `ResolveName(name)` request. The home domain returns the Owner's public identity (`id`) — or `NameNotFound` if no such Owner exists.
4. **Contact the Owner.** Use the resolved identity for Grant exchange, presence subscription, or messaging.

Name resolution is a public operation — it doesn't require a Grant. Knowing someone's name and resolving it to an identity is like looking up a phone number in a directory. The identity alone grants nothing — you still need a `presence` or `contact` Grant to observe or message them.

**Caching.** Name-to-identity mappings are stable (Owners don't change their cryptographic identity). Clients and domains can cache the mapping indefinitely. If an Owner re-keys (key compromise, migration), the old mapping returns `IdentityChanged(new_id)` — the cache invalidates and the caller learns the new identity.

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

#### Messaging

The `message` scope authorizes the grantee to send messages to the grantor. A message is a Leden method call on the Owner's identity at the home domain:

```
SendMessage(to: owner_id, content: bytes, content_type: string)
```

| Field | Description |
|-------|-------------|
| `to` | Recipient Owner identity |
| `content` | Opaque bytes. Interpretation determined by `content_type`. |
| `content_type` | MIME-style type tag: `text/plain`, `application/gdl+msgpack`, etc. |

**Delivery semantics: at-most-once.** The home domain delivers the message to the recipient if reachable. No guaranteed delivery, no persistent queue obligation. If the recipient is offline, the home domain's queuing policy applies (see [Unreachable Owners](#unreachable-owners)). The sender receives `Delivered`, `Queued`, or `Unreachable`.

**Size limit: 64KB per message.** Messages are for communication, not file transfer. Large payloads use Leden content store references inside the message body.

**Rate limiting.** Law 5 applies. The home domain enforces message rate limits per sender. Default: 10 messages/second per sender-recipient pair. This prevents spam while allowing real-time conversation.

**Content types are conventions.** `text/plain` is universal — every client can render it. Richer types (`application/gdl+msgpack` for structured game data, domain-specific types) degrade to "message received, type not supported" on clients that don't recognize them. Same principle as email MIME types.

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

#### Group Object

The group Object lives on the administrator's home domain (or any domain the administrator chooses to host it). It's observable — members subscribe to it for membership changes.

| Property | Description |
|---|---|
| `type` | `group` |
| `name` | Group display name |
| `admin` | Owner identity of the administrator |
| `members` | Set of Owner identities |
| `roles` | Optional. Map of Owner → role string (`admin`, `moderator`, `member`) |

The group Object is the source of truth for membership. When the administrator adds a member, the Object updates, observers see the delta, and the new member receives their Grants. When a member is removed, their Grants are revoked and the Object updates.

#### Group Messaging

A message to a group is a message to the group Object's hosting domain:

```
SendMessage(to: group_object_ref, content: bytes, content_type: string)
```

The hosting domain is responsible for fan-out — delivering the message to all current members. Fan-out uses each member's home domain for routing (same as individual messaging). The sender sends one message; the hosting domain multiplies it.

| Concern | How it works |
|---|---|
| Delivery | At-most-once per member, same as individual messages |
| Rate limiting | Law 5 applies per sender per group |
| Ordering | Messages carry sequence numbers on the group Object. Members see the same order. |
| Offline members | Member's home domain queuing policy applies, same as individual messages |
| Large groups | Hosting domain's fan-out capacity is the bottleneck. Domain policy can cap group size. |

**Cross-domain groups.** Members can be on different home domains. The group Object's hosting domain routes to each member's home domain. This is the same pattern as any cross-domain interaction — bilateral sessions, capability-gated delivery. A group with members across 10 domains means the hosting domain maintains sessions with 10 home domains for fan-out.

#### Group Presence

A group's presence is the aggregated presence of its members. The group Object can be observed for member presence:

- Observe the group Object → receive membership changes + per-member online/offline status
- The hosting domain aggregates presence from members' home domains
- Individual member domain sets are NOT exposed through group observation (only the member's home domain knows that). Group presence shows: member X is online/offline. Where they are is gated by individual `presence` Grants between members.

### Cross-Domain Consistency

Standard Grants carry their type tag (`presence`, `contact`, `group`) so receiving domains know the convention. A `contact` Grant issued on Domain A is recognized on Domain B with the same semantics — Domain B knows what `observe_presence` + `message` means because the convention is standardized.

Without this, a Grant scoped `[0x42, 0x07]` on Domain A means nothing to Domain B. Named conventions give Grants portable semantics.

## Message Encryption

Messages route through home domains. Home domains can read them unless they're encrypted. The protocol requires that message encryption is available, but does not mandate a specific cryptographic scheme.

### Requirements

1. **End-to-end encryption must be possible.** The protocol must support messages that the home domain cannot read. This is a structural requirement — the message `content` field is opaque bytes, and the protocol never requires the home domain to inspect content for routing or delivery.

2. **The specific scheme is not specified.** Cryptographic standards evolve. Mandating a specific protocol (Signal, MLS, etc.) in a federation spec creates a migration problem when better schemes emerge. Instead, the protocol provides the mechanism; implementations choose the cryptography.

3. **Key exchange uses existing primitives.** Owners have cryptographic identities ([key hierarchy](PRIMITIVES.md#key-hierarchy)). Key exchange for E2E encryption uses these identities — the Owner's device keys are the foundation for establishing encrypted channels. The specific key exchange protocol (X3DH, post-quantum KEM, etc.) is negotiated between clients.

### How It Works

The `contact` Grant establishes that two Owners can communicate. The encrypted channel setup happens at first message (or on Grant acceptance):

1. Both Owners' device keys are public (published at their home domains as part of their identity).
2. The sending client derives a shared secret using the recipient's public device key and its own private device key.
3. Messages are encrypted client-side before submission to the home domain.
4. The home domain routes the encrypted blob — it sees `to`, `content_type`, and opaque `content`. It cannot decrypt.
5. The receiving client decrypts using the shared secret.

**Content type signals encryption.** An encrypted message uses a content type like `application/encrypted+X` where X identifies the scheme. Clients that support the scheme decrypt and render. Clients that don't show "encrypted message, unsupported scheme." The home domain doesn't need to know the scheme — it routes the opaque blob.

**Group encryption.** For group messages, the hosting domain fans out encrypted blobs. Each member receives the same ciphertext. Group encryption schemes (MLS, Sender Keys, etc.) handle the multi-party key management. The protocol's role is the same: route opaque bytes, let clients handle cryptography.

### What the Protocol Guarantees

- Message content is opaque bytes at every routing layer. No protocol operation requires content inspection.
- Home domains and relays can be honest-but-curious — they route correctly but learn nothing from content.
- Metadata (sender, recipient, timestamp, size) is visible to routing domains. Metadata privacy is a harder problem and out of scope — it requires onion routing or mixnets, which are transport-layer concerns (Leden), not federation-layer concerns (Allgard).

### What the Protocol Does Not Guarantee

- That a specific encryption scheme is secure. That's the scheme's job.
- Metadata privacy. The home domain knows who talks to whom and when.
- Forward secrecy, post-compromise security, deniability. These are properties of specific schemes, not the routing protocol.

## Presence Privacy

A `presence` Grant reveals which Domains the Owner is currently active on. This is useful for routing (contact them directly on Domain B instead of routing through home) but reveals movement patterns (Owner was on Domain X at 3am, moved to Domain Y at 9am).

Two scopes, chosen by the grantor when issuing the Grant:

| Scope | What the observer sees | Use case |
|---|---|---|
| `observe_liveness` | Online or offline. Nothing more. | Casual contacts — "is my friend online?" |
| `observe_presence` | Full domain set (which Domains, when) | Close contacts — "where is my friend so I can join them?" |

The grantor chooses per-grantee. Close friends get `observe_presence`. Acquaintances get `observe_liveness`. The choice is part of the Grant's scope field — same mechanism, different scope strings.

**Default: `observe_liveness`.** When the spec says "a `presence` Grant" without qualification, it means `observe_liveness`. Full presence requires explicit `observe_presence` scope. This is the privacy-safe default — you have to opt in to revealing your location across domains.

The `contact` Grant includes `observe_liveness` by default (not `observe_presence`). An Owner who wants a contact to see their full domain set upgrades the Grant scope explicitly.

## What This Doesn't Cover

- **Spatial location within a domain.** Presence says "Owner is on Domain X." It doesn't say "Owner is at coordinates (3,4) in region Y." Spatial protocols build on top of presence.
- **Profile content schema.** The Owner's [profile Object](PRIMITIVES.md#profile) is defined as a mechanism in Allgard. The content schema (which fields, what they mean) is defined in [GDL](../gdl/GDL.md#owner-profile-schema).
- **Voice or media channels.** Real-time media between Owners is an application feature using Leden transport. Presence tells you who's reachable; media setup is a separate capability negotiation.
- **Notification preferences.** Whether an Owner wants to be disturbed, what channels they prefer — local policy stored at the home domain, not a federation concern.
- **Metadata privacy.** Who talks to whom and when is visible to routing domains. Hiding this requires onion routing or mixnets at the transport layer (Leden), not the federation layer.

## Relationship to Existing Specs

| Spec | Relationship |
|------|-------------|
| [PRIMITIVES.md](PRIMITIVES.md) | Extends the Owner primitive with presence state. Uses the Grant primitive for standard conventions. |
| [CONSERVATION.md](CONSERVATION.md) | No changes. Presence is not a conservation law. Law 5 (bounded rates) applies to presence updates and messaging like any other operation. |
| [TRANSFER.md](TRANSFER.md) | Leased transfer already implies Owner presence on visited domains. Presence formalizes what was implicit. |
| [TRUST.md](TRUST.md) | Presence enables richer trust signals — Owners that are consistently reachable build reputation faster than ghosts. |
| [Leden observation](../leden/observation.md) | Presence updates are delivered via Leden observation. All existing observation semantics (backpressure, reconnection, filtered observation) apply. |

## Resolved

**Presence privacy.** Two scopes: `observe_liveness` (online/offline only, the default) and `observe_presence` (full domain set). See [Presence Privacy](#presence-privacy). The grantor chooses per-grantee. Default is the privacy-safe option.

**Home domain migration.** Specified in [PRIMITIVES.md](PRIMITIVES.md#home-domain-migration). During cooperative migration, the old home sends `HomeMigrated(new_home)` to all presence observers — clients reconnect to the new home automatically. Name resolution redirects (`OwnerMigrated`) handle cached addresses. During uncooperative migration, the Owner notifies contacts directly through existing sessions. Presence subscriptions at the old home eventually fail; contacts that have the Owner's cryptographic identity can re-subscribe at the new home.
