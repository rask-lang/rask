<!-- id: allgard.primitives -->
<!-- status: proposed -->
<!-- summary: The six federation primitives -->

# Primitives

Six primitives. Everything in the federation composes from these.

## Object

An opaque blob with a content-addressed ID, a type tag, and an owner.

An Object is simultaneously three things:
- **Data** — it has state (the blob)
- **Actor** — it receives messages (Transforms), has private state, no shared memory
- **Capability** — holding a reference to it IS your permission to interact with it

Objects are content-addressed: the ID is derived from the content. This means:
- Deduplication is free
- Integrity verification is free
- References are unforgeable (you can't guess a valid ID)

Every Object has exactly one Owner at any point in time (Conservation Law 2). Ownership transfer is atomic.

### Properties

| Property | Description |
|----------|------------|
| `id` | Content-addressed identifier (hash of content + type + metadata) |
| `type` | Tag describing the object's schema/interface |
| `owner` | The Owner identity that has authority over this object |
| `domain` | The Domain currently hosting this object |
| `content` | Opaque bytes. Interpretation determined by `type`. |

## Owner

An identity that holds capabilities — references to Objects.

An Owner can:
- Authorize Transforms on Objects it owns
- Receive Grants from other Owners
- Delegate authority via Grants to other Owners
- Revoke Grants it has issued

Owners are cryptographic identities. The specific scheme (ed25519 keys, DIDs, etc.) is a protocol decision, not a primitive concern.

An Owner is *not* a person. A person may control multiple Owners. An automated system can be an Owner. The federation doesn't care about the entity behind the key.

### Properties

| Property | Description |
|----------|------------|
| `id` | Cryptographic identity (public key or derived identifier) |
| `name` | Human-readable identifier, unique within the home domain. Globally addressed as `name@home_domain`. |
| `kind` | Advisory type tag: `individual`, `system`, `group`, `service`. Not enforced — domains can use it for policy (rate limits, trust defaults, display). |
| `home_domain` | The Domain authoritative for this Owner's identity |
| `profile` | Optional. Object reference to the Owner's profile Object (see [Profile](#profile)). |

### Name

Every Owner has a name — a human-readable identifier, unique within its home domain. The global form is `name@home_domain`, like email addresses. Globally unique by construction, no registry needed.

Names are how Owners refer to each other across the federation. Without standardized naming, every cross-gard interaction would use raw cryptographic IDs. That's fine for machines. It's unusable for anything involving humans.

The home domain is authoritative for name uniqueness within its namespace. Name resolution goes through the home domain, same as email MX resolution.

**Names are not secrets.** An Owner's name is public — it's how you're addressed. Knowing someone's name doesn't grant any capability. You still need a Grant to observe their presence or contact them.

### Kind

Advisory type tag. The federation doesn't enforce it — an Owner claiming `individual` might be a bot. But kind serves two purposes:

1. **Policy defaults.** A domain might apply different rate limits to `system` Owners (higher throughput, lower interactivity) vs `individual` Owners. A `service` Owner connecting at 3am to execute 1000 transfers looks normal. An `individual` doing that looks suspicious.

2. **Interaction expectations.** A `group` Owner (guild, organization) has members and delegation. A `service` Owner (automated trading, monitoring) is expected to be always-on. These expectations aren't enforced — they inform how domains and other Owners interact.

| Kind | Typical use |
|------|------------|
| `individual` | A person. May be offline. Has social relationships. |
| `system` | Automated process. Expected to be always-on. High throughput, low interactivity. |
| `group` | Multi-member entity (guild, organization). Has an administrator. Delegates via Grants. |
| `service` | Provides functionality to other Owners (marketplace, exchange, hosting). Publicly reachable. |

Kind is self-declared and immutable once set. Changing kind requires a new Owner identity. I considered making it mutable but rejected it — a `system` that becomes an `individual` mid-session breaks every policy assumption. If you need a different kind, create a new Owner.

### Profile

An Owner's profile is an Object — a regular Object with type tag `owner_profile`, owned by the Owner, published at the home domain. The profile carries identity metadata beyond the fundamental properties (name, kind).

**Why an Object, not more primitive properties?** Because the boundary between "fundamental" and "application-specific" depends on context. Name and kind are universal — every federated system needs them. Avatar, bio, display name — almost universal, but not quite. Game class, sensor type — domain-specific. Putting everything in the primitive forces every system to carry fields it doesn't use. An Object with a typed schema lets each context carry what it needs.

**The profile Object is observable.** Other Owners with a `presence` or `contact` Grant (see [PRESENCE.md](PRESENCE.md#standard-grants)) can observe the profile via Leden observation at the home domain. No transfer needed — read it from the authoritative source, cache locally. Content-addressed, so cache invalidation is free (content changes = new hash = refetch).

#### Well-Known Fields

The profile Object's content is a typed map. [GDL](../gdl/GDL.md#owner-profile-schema) defines the standard schema — well-known fields like `display_name`, `avatar`, `bio`, `links`, `pronouns`, and `locale`. This is the same pattern as HTTP headers: a small set of well-known names with defined semantics, plus arbitrary extensions via namespaced keys.

The split is deliberate: Allgard defines the mechanism (profile is an Object, observable, at home domain). GDL defines the content (what fields exist, what they mean). Content description is GDL's job.

#### Graceful Degradation

Domains render what they recognize, ignore what they don't. GDL's first design principle — "ignore what you don't understand" — applies directly to profiles.

| What the domain recognizes | What it shows |
|---|---|
| Full GDL profile schema + domain extensions | Rich profile — avatar, bio, custom fields |
| Standard GDL profile fields only | Avatar, bio, display name |
| No profile support | `name` and `kind` from the Owner primitive |

Every step is functional. A supply chain system that doesn't render avatars still has `name` and `kind`. A game domain that adds `class` and `level` fields can render rich character profiles. Neither breaks when encountering the other's profiles — unknown fields are silently ignored.

#### Domain-Specific Extensions

Domains add custom fields to the profile Object. A game domain might add `class`, `level`, `guild`. A supply chain domain might add `facility_type`, `capacity`. These fields travel with the profile Object and are available to any domain that recognizes them.

Custom fields use a namespaced key convention to avoid collisions: `domain_name.field_name`. A game domain `northgard` adding a class field uses `northgard.class`. Domains never need to coordinate field names — the namespace prevents collisions by construction.

Namespaced fields that a domain doesn't recognize are preserved but not rendered. If a player from `northgard` visits `eastgard`, and `eastgard` doesn't know about `northgard.class`, the field survives in the profile Object — it's just not displayed. When the player returns to `northgard`, the field is still there.

### Home Domain

Every Owner has a home domain — the domain that is authoritative for their identity and primary inventory. An Owner can operate in other domains, but their home domain is the root of trust for their identity.

The home domain is where your stuff lives by default.

### Presence

An Owner's presence is the set of Domains where the Owner currently has active sessions. Presence is observable state — other Owners with appropriate Grants can subscribe to presence changes. The home domain is the canonical observation point.

See [PRESENCE.md](PRESENCE.md) for the full spec: observability, reachability, and standard relationship Grant conventions.

### Leased Transfer

When a player visits another domain, objects don't transfer permanently — they transfer on a **lease**. The lease is a time-limited escrow built from existing primitives (Transform + Grant + expiry):

> "These objects are hosted by Domain B. If the lease isn't renewed within N hours, they return to the home domain."

The player's client maintains sessions with both the home domain and the visited domain. The home domain issued the lease, so the home domain can revoke it.

**Normal disconnect:** Player goes offline → home domain detects session loss → home domain revokes the lease → visited domain transfers objects back. This takes seconds, not hours. The revocation uses the existing membrane pattern — the Grant gets switched off.

**Catastrophic disconnect:** Both the player and the home domain are unreachable. Only then does the lease timeout kick in (hours/days — configurable). This is the safety net, not the normal path.

Lease renewal is automatic and invisible — like a DHCP lease, nobody thinks about it.

**Why transfer at all?** Because game logic needs low latency. If objects stayed on the home domain and the visited domain operated on them remotely, every sword swing would be a cross-domain round trip. Leased transfer gives the visited domain local access for game logic while the home domain retains recovery authority.

### Exit Scenarios

| Scenario | What happens | Speed |
|----------|-------------|-------|
| **Normal exit** | Player leaves Domain B. Objects transfer home immediately. | Instant |
| **Sudden disconnect** | Home domain detects session loss, revokes lease. Visited domain transfers objects back. | Seconds |
| **Visited domain goes dark** | Home domain can't reach visited domain to revoke. Lease timeout expires, home domain recovers from Proof chain. | Hours (timeout) |
| **Both go dark** | Lease timeout is the only mechanism. Objects recover when home domain comes back online. | Hours to days |
| **Home goes dark, player active** | Player keeps playing on visited domain. Lease stays active — visited domain has no reason to evict an active, authenticated player. | No disruption |
| **Home goes dark, player disconnects** | Nobody to revoke, nobody to renew. Lease timeout is the safety net. Objects stay on visited domain until home comes back or timeout expires. | Hours to days |
| **Home gone permanently** | Backup home domain takes over. See below. | Depends on backup |

### Home Domain Failure

Your home domain is your root of trust. If it's temporarily down, objects on visited domains are safe — the lease holds. If it's permanently gone, your identity and home-stored objects go with it unless you have a backup.

**Backup home domain.** Every player should have one. It's a second domain that mirrors your identity and inventory Proof chains in real-time. The backup domain holds a read-only replica. If the primary goes dark, the backup can:

1. Take over as the new home domain
2. Revoke outstanding leases (it has the Proof chains showing what transferred out)
3. Accept returning objects
4. Issue new leases for future visits

This is a Grant from the player to the backup domain — scoped to mirror and recover, not to use or transfer. The backup can't touch your stuff until the primary is declared dead (configurable timeout, or player-initiated failover).

**This should be a first-class protocol feature, not a pattern.** The Owner primitive should have an optional `backup_home` field. The runtime should handle replication automatically. "Choose a backup home" during account setup — same as setting up 2FA. Not required, but the UI should make it the path of least resistance.

Without a backup, home domain failure is permanent loss. That's the honest tradeoff. But with a backup, it's a recoverable event — same as a disk failure with a RAID mirror.

### Key Compromise Propagation

When an Owner's key is compromised, the home domain must propagate revocation to every domain where the Owner has active sessions, outstanding Grants, or leased objects. This is the hardest revocation case — it's time-critical and cross-domain.

**Revocation flow:**

1. **Owner or home domain detects compromise.** The Owner reports a compromised key (out-of-band — new key signed by backup key, admin action, etc.), or the home domain detects anomalous behavior (impossible concurrent sessions, operations from conflicting locations).

2. **Home domain issues `KeyRevocation(owner_id, compromised_key, evidence, new_key)`** to all domains it has bilateral relationships with. This is a broadcast, not targeted — the home domain may not know every domain the Owner visited (Grants can chain through intermediaries). The message includes:
   - The Owner identity being revoked
   - The compromised public key
   - Evidence: signed statement from the backup key, or from the home domain's admin key
   - The replacement public key (if available) or `null` (Owner disabled pending re-keying)

3. **Receiving domains apply synchronous revocation strategy** (see [Revocation](#revocation)). Key revocation is always synchronous — the strictest strategy, regardless of what was negotiated for other Grant types.

4. **Receiving domains propagate to their trading partners.** If Domain B received a Grant from the compromised Owner and delegated a sub-Grant to Domain C, Domain B revokes the sub-Grant and forwards the `KeyRevocation` to Domain C. Propagation follows the Grant delegation graph.

**Propagation rules:**

| Situation | Action |
|-----------|--------|
| Active session from compromised key | Terminate immediately. All in-flight operations rejected. |
| Outstanding Grants from compromised Owner | Revoke all. Sub-Grants revoked transitively. |
| Leased objects from compromised Owner | Freeze in place. No mutations allowed until new key confirms or lease expires. Objects return to home domain on lease expiry if no new key is presented. |
| Objects transferred *to* compromised Owner (completed transfers) | No clawback. Completed transfers are final (Law 4 — sequential history). The compromised key may have already moved them. The home domain's recourse is through the replacement key. |
| Pending transfers involving compromised Owner | Abort. Escrow releases back to source. |

**Timing.** Key revocation is the one case where I accept the cost of synchronous cross-domain coordination. A compromised key can cause unbounded damage if revocation is eventual. The target: all direct trading partners notified within 5 seconds. Transitive propagation (via Grant delegation chains) adds latency per hop — the depth of the Grant graph determines total propagation time, but each hop is bounded by the synchronous revocation timeout (default: 10s).

**Without a backup key.** If the Owner has no backup key and the home domain's admin issues the revocation, the Owner identity is effectively dead. All Grants revoked, all sessions terminated, leased objects frozen until lease expiry. The Owner must create a new identity and re-establish relationships from scratch. This is intentionally harsh — it incentivizes backup keys.

**Replay protection.** `KeyRevocation` messages include a monotonic sequence number per Owner (stored at the home domain). Receiving domains reject revocations with a sequence number ≤ the last seen revocation for that Owner. This prevents an attacker from replaying an old revocation to disrupt a legitimate key rotation.

### Owner Wallet

The ultimate fallback: you hold your own proof of ownership locally.

A wallet is a serialized file containing:
- **Owner private key** — your cryptographic identity
- **Object contents** — the bytes of every object you own
- **Proof chains** — every Transform from minting to current state, per object. Signatures, causal links, timestamps.
- **Minting scripts** — the content-addressed Raido scripts that created each object

This is everything a domain needs to mechanically verify your ownership. Re-execute the minting scripts, check the signatures, walk the causal chain. No server has to be online. No domain has to vouch for you. The math speaks for itself.

**The wallet is a file.** Put it on a thumb drive, back it up to cold storage, print the key on paper. Your ownership exists independently of any domain. This is the same principle as a crypto wallet, but without a blockchain — the Proof chain IS the ledger, and it's self-contained per owner.

#### When You Need It

The wallet is the nuclear option for recovery:

| Scenario | Primary recovery | Wallet recovery |
|----------|-----------------|-----------------|
| Home domain temporarily down | Wait for it to come back | Not needed |
| Home domain permanently gone, backup exists | Backup takes over | Not needed |
| Home domain permanently gone, no backup | **Wallet is the only recovery path** | Present wallet to any domain |
| All domains you've ever used go dark | Nothing else works | Wallet proves ownership to any new domain |

#### How Recovery Works

1. Player presents wallet to a new domain
2. Domain verifies the Proof chains mechanically — re-executes minting scripts, checks signatures, validates causal ordering
3. If everything checks out, the domain accepts the objects and registers the player as their Owner
4. The player's new home domain starts fresh Proof chains from this point forward

#### Witnessed Recovery

A wallet alone is a local copy, and local copies can be presented twice. Without a structural fix, double-spend is a real hole. Gossip-based detection isn't enough — it's after-the-fact, and the damage is done.

The fix: **wallet recovery requires witnesses.**

Objects don't exist in a vacuum. Every object has a history — it was minted somewhere, transferred through domains, traded with counterparties. Those counterparties have partial views. The Proof chain in the wallet references them. They're the witnesses.

**Recovery protocol:**

1. Player presents wallet to a new domain (the "recovering domain")
2. Recovering domain verifies the Proof chains mechanically (signatures, causal links, minting scripts)
3. Recovering domain contacts **witnesses** — domains referenced in the Proof chains as counterparties to recent Transforms involving these objects
4. Each witness checks: "Do I have records of these objects? Has anyone else already claimed recovery for them? Is the Proof chain consistent with what I saw?"
5. Recovery requires **N-of-M witnesses** to co-sign: "I last saw these objects belonging to this Owner, and I haven't seen them claimed elsewhere"
6. Only after quorum does the recovering domain accept the objects

**Why this prevents double-spend:**

The attacker presents the same wallet to Domain X and Domain Y simultaneously. Both contact the same witnesses (the witnesses are determined by the Proof chain, not chosen by the player). The first domain to get quorum wins. When the second domain contacts the same witnesses, they respond: "Already co-signed recovery for these objects to Domain X." The second claim is rejected.

The race window is bounded by how fast witnesses respond — not gossip propagation, but direct request-response. Witnesses have every incentive to respond honestly: their records are verifiable (they have their own Proof chains for the transactions they witnessed), and lying about recovery is detectable fraud.

**What if witnesses are down too?**

If enough witnesses are offline that quorum can't be reached, recovery is delayed until they come back. This is the honest tradeoff: you can't recover objects faster than your witnesses can confirm them. In the catastrophic case where most witnesses are permanently gone, the recovering domain accepts a lower quorum with a longer provisional hold and wider gossip announcement.

| Witnesses available | Recovery behavior |
|--------------------|--------------------|
| Full quorum (N of M) | Immediate recovery |
| Partial quorum | Provisional recovery + extended hold + gossip announcement |
| No witnesses reachable | Recovery blocked until witnesses return |

**What the wallet provides vs. what witnesses provide:**

- **Wallet** proves: "These objects existed and I owned them at this point in time" (cryptographic, self-contained)
- **Witnesses** prove: "Nobody else has claimed these objects since then" (requires liveness, prevents double-spend)

Both are needed. The wallet alone is proof of historical ownership. Witnesses confirm that history hasn't been superseded. Together, they're a complete recovery mechanism without a global ledger.

#### Wallet Sync

The wallet should stay current. The runtime should sync the wallet file automatically:
- After every Transform that affects the player's objects
- After every cross-domain transfer
- After lease creation/revocation

This is a local operation — writing to a file on the player's machine. No network round trip. The wallet file grows over time (Proof chains accumulate), but compaction is possible: once a Proof chain is accepted by a trusted domain, the domain's acceptance Proof replaces the full chain.

#### What This Means

The player's sovereignty is complete. Not "sovereignty as long as your home domain is up" — actual sovereignty. Your identity is your key. Your ownership is your Proof chains. Everything else is convenience layered on top.

- Home domain: convenient, fast, handles leases and gossip for you
- Backup domain: safety net for home domain failure
- Wallet: ultimate fallback, works with zero infrastructure

Each layer is less convenient and more sovereign. The player chooses how much infrastructure to trust.

### Why This Works

The lease model means objects are always recoverable. The worst case (visited domain goes dark) loses recent mutations — not the objects themselves. The home domain has the Proof chain showing what left and can reconstruct from there.

This composes entirely from existing primitives. The escrow transform from [transfer routing](../allgard/README.md#cross-domain-transfer-routing) already describes conditional transfers with timeouts. Leased visiting is the same mechanism, applied to player travel instead of intermediary routing.

## Domain

An authority boundary. A gard that hosts Objects and enforces local rules.

A Domain is:
- **Trust boundary** — code running inside a Domain trusts other code in that Domain. Code across Domains does not trust each other by default.
- **Authority root** — the Domain is the final arbiter for Objects it hosts
- **Rule enforcer** — the Domain enforces its own rules (rate limits, content policies, application logic) on top of the universal Conservation Laws

Domains map to E's concept of a "machine" (trust boundary), not a "vat" (execution unit). A Domain may contain many execution units internally.

### Sovereignty

Each Domain is sovereign over its hosted Objects. It can:
- Define custom object types and rules
- Set rate limits and policies
- Accept or reject incoming transfers
- Run its own application logic

What it cannot do:
- Violate Conservation Laws
- Modify Objects it doesn't host
- Forge capabilities it wasn't granted

### Federation

Domains federate. Any Domain can communicate with any other Domain via Leden's capability protocol. There's no central authority, no global state, no master server. Domains discover each other through gossip and establish bilateral capability relationships.

## Transform

A proposed operation on an Object. A message send.

A Transform hasn't happened yet. It's a request: "I want to do this to this object." The hosting Domain validates and applies it (or rejects it).

### Operations

| Operation | Description |
|-----------|------------|
| `create` | Bring a new Object into existence. Must be backed by a Raido script ([verifiable minting](CONSERVATION.md#verifiable-minting)). |
| `mutate` | Change an Object's content |
| `transfer` | Move an Object to a new Owner. Cross-domain transfers use the [escrow protocol](TRANSFER.md). |
| `split` | Divide an Object into parts (fungible assets) |
| `merge` | Combine Objects into one (fungible assets) |
| `destroy` | Remove an Object from existence (burning). Must be backed by a Raido script ([verifiable minting](CONSERVATION.md#verifiable-minting)). |

### Promise Pipelining

Transforms support promise pipelining: you can send a Transform to the result of a Transform that hasn't resolved yet. This eliminates round-trip latency for chains of operations.

Example: "Transfer asset to winner of auction" doesn't need to wait for auction resolution to queue the transfer. The transfer references the promise of the auction's result.

Stolen directly from E/CapTP. Essential for distributed performance.

### Causal Ordering

Every Transform references the state it's operating on (Conservation Law 4). This means:
- No time-travel exploits
- No replay attacks
- No fork-based duplication
- Every mutation forms a DAG, not just current state

## Proof

Evidence that a Transform is valid.

Within a Domain, Proofs are whatever the Domain's internal validation requires. The interesting case is cross-domain: when Domain A wants to convince Domain B that a Transform is legitimate.

A Proof must establish:
- The Transform was authorized by the Object's Owner (signature)
- The Transform references a valid prior state (causal link)
- Any Domain-specific rules were satisfied

Proofs are the trust-bootstrapping mechanism. When two Domains that have never interacted want to exchange Objects, Proofs are how they verify legitimacy without trusting each other.

### Verifiable Proofs

For Transforms backed by [Raido](../raido/) scripts, a Proof includes the script hash, inputs, and outputs. The receiving Domain fetches the script and re-executes — determinism guarantees identical results. This turns a trust-based Proof into a mechanically verifiable one.

**Required for mint/burn.** Every `create` and `destroy` Transform must include a verifiable Proof. This is not negotiable — it's how [Conservation Law 1](CONSERVATION.md#verifiable-minting) is structurally enforced. A domain that can't produce a verifiable minting Proof can't mint.

**Optional for other transforms.** General transforms (transfer, mutate, split, merge) can use trust-based Proofs. Domains that want stronger guarantees negotiate "verifiable-transform" as a Leden capability. See [Verifiable Transforms](README.md#verifiable-transforms).

## Grant

Scoped, optionally time-limited authority delegation. An attenuated capability.

A Grant lets an Owner delegate specific authority over specific Objects to another Owner, without transferring ownership. The recipient can exercise the granted authority but cannot escalate it.

### Properties

| Property | Description |
|----------|------------|
| `grantor` | The Owner delegating authority |
| `grantee` | The Owner receiving authority |
| `scope` | What operations are permitted (e.g., read-only, mutate specific fields) |
| `target` | Which Objects the Grant applies to |
| `expiry` | Optional time limit. `None` means revoke-only. |
| `revocable` | Whether the grantor can revoke. Default: yes. |

### Attenuation

Grants can only narrow, never widen. If Owner A grants Owner B read+write on an Object, Owner B can grant Owner C read-only — but not read+write+transfer. Authority flows downhill.

### Revocation

Revocation is built in, not optional. The mechanism is the membrane pattern: the Grant is a wrapper that can be switched off. When revoked, all further Transforms through that Grant are rejected.

Revocation is **eventually consistent** in a distributed system. There's unavoidable latency between "revoke" and "all domains know it's revoked." The protocol must handle the window where a revoked Grant is still being exercised somewhere.

Three strategies, with defaults that apply when domains haven't negotiated:

| Strategy | Default for | Behavior |
|----------|-------------|----------|
| **Optimistic** | Read-only grants, observation | Allow during window, reconcile after |
| **Pessimistic** | Mutation grants, delegation grants | Liveness check before honoring |
| **Synchronous** | Security-critical (key revocation, admin) | Block until all holders acknowledge |

Defaults exist so the system is safe without negotiation. Two domains that haven't discussed revocation policy still get reasonable behavior. When domains disagree on strategy for a Grant class, the stricter strategy wins — a domain can always be more cautious than required, never less.

See [Leden protocol spec](../leden/protocol.md#revocation) for propagation SLAs, lease renewal failure behavior, and the full negotiation mechanism.

### Third-Party Handoff

A Grant enables third-party introduction: Owner A sends Owner B a reference to an Object on Domain C. Owner B connects directly to Domain C using the Grant. Cross-domain object introduction without a central broker.

This should be a named operation in the protocol, not an implicit consequence of Grant semantics.

## How They Compose

A typical cross-domain interaction:

1. **Owner A** on **Domain X** holds an **Object**
2. Owner A creates a **Grant** giving **Owner B** transfer authority over the Object
3. Owner B submits a **Transform** (transfer Object to Owner B) to **Domain X**
4. Domain X validates the Transform against the Grant and the Conservation Laws
5. Domain X produces a **Proof** of the transfer
6. Owner B's home **Domain Y** receives the Proof and registers the Object in Owner B's inventory

Every step uses only the six primitives. No special cases.
