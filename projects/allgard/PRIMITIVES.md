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

### Home Domain

Every Owner has a home domain — the domain that is authoritative for their identity and primary inventory. An Owner can operate in other domains, but their home domain is the root of trust for their identity.

The home domain is where your stuff lives by default.

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
| `transfer` | Move an Object to a new Owner |
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

Revocation is **eventually consistent** in a distributed system. There's unavoidable latency between "revoke" and "all domains know it's revoked." The protocol must handle the window where a revoked Grant is still being exercised somewhere. Options:

1. **Optimistic**: allow operations during the window, reconcile after
2. **Pessimistic**: require liveness check before honoring a Grant
3. **Hybrid**: optimistic for low-value, pessimistic for high-value

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
