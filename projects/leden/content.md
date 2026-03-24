<!-- id: leden.content -->
<!-- status: proposed -->
<!-- summary: Content-addressed blob storage and lazy fetching -->

# Content Store

How Leden handles data behind references. The problem: object references are small, but the data they point to can be arbitrarily large. A reference to a 3D model shouldn't force you to download 50MB just to hold it.

## The Split

Every piece of data in the system is either a **reference** or a **blob**.

| Thing | Size | Identity | Mutability |
|-------|------|----------|------------|
| **Reference** | Tiny (hash + metadata) | Content-addressed | Immutable (new content = new hash) |
| **Blob** | Arbitrary | Hash of content | Immutable |

References travel through the capability protocol (Layer 2-3). Blobs travel through the content store. Holding a reference does not require having the blob. You fetch the blob when you need it.

## Content Addressing

Blob identity is the hash of the content. This gives you:

- **Deduplication** — two endpoints storing the same data have the same hash. Free.
- **Integrity** — fetch a blob, hash it, compare. Tampered = wrong hash. Free.
- **Unforgeability** — can't construct a valid hash without having the content. Same property capabilities rely on.
- **Immutability** — changing content changes the hash, which means it's a different blob. There's no "update in place."

Hash algorithm is a protocol decision (SHA-256 is the obvious default). Must be fixed per protocol version — mixed hashes are a nightmare.

## Fetching

A content fetch is a request: "give me the bytes for this hash." The response is either the bytes or "I don't have it."

```
Endpoint A                          Endpoint B
    |                                   |
    |  ContentRequest(hash)             |
    |──────────────────────────────────>|
    |                                   |
    |  ContentResponse(bytes)           |
    |<──────────────────────────────────|
    |                                   |
```

This fits into Leden's existing promise model. A content fetch returns a promise. The promise resolves to bytes (or an error). You can pipeline operations on the result — "fetch this blob, then decode field X from it" is one round trip, not two.

### Chunking

Large blobs are split into fixed-size chunks (e.g. 256KB). Each chunk is independently content-addressed. A blob's hash is the hash of its chunk list (a Merkle tree).

Why:
- **Resumable transfers** — network drops, you pick up where you left off
- **Parallel fetching** — request chunks from multiple sources simultaneously
- **Partial fetching** — if you only need the first 1MB of a 50MB blob, fetch those chunks
- **Dedup at chunk level** — two blobs that share a common prefix share chunks

### Sources

A blob doesn't have a single canonical location. Any endpoint that has it can serve it. This means:

- **Origin** — the endpoint that created the blob. Always has it (unless it's been garbage collected).
- **Cache** — any endpoint that fetched it previously and hasn't evicted it.
- **Peer** — another endpoint that happens to have it. Fetch from whoever's closest.

The protocol doesn't mandate a specific resolution strategy. An endpoint can ask the origin, ask known peers, or use a separate discovery mechanism. The content store defines the fetch operation; routing is policy.

## Pinning and Eviction

Endpoints decide what to keep locally. The content store doesn't force replication.

- **Pin** — "I want this blob to stay local." Prevents eviction.
- **Evict** — "I don't need this anymore." Frees local storage. The blob still exists elsewhere.
- **Cache** — "Keep this around if there's room, evict if storage is tight." LRU or whatever.

This is entirely local policy. The protocol doesn't dictate storage strategy. A resource-constrained embedded device might cache nothing. A game server might pin all assets for its active region. A CDN might cache everything.

## Relationship to Object Layer

Layer 3 (Object) deals with references, method calls, and promises. The content store is how references resolve to actual data.

An object reference contains:
- Capability token (Layer 2) — authority to interact
- Type information (Layer 3) — what methods/interface
- Content hash (Content Store) — where the data lives

The first two are always present in the reference. The content hash is a pointer — you follow it when you need the bytes.

```
┌──────────────────────────────────────────┐
│  3. Object        References, calls      │
│     └── content hash ──→ Content Store   │
├──────────────────────────────────────────┤
│  2. Capability    Authority, delegation  │
├──────────────────────────────────────────┤
│  1. Session       Multiplexing, reconnect│
├──────────────────────────────────────────┤
│  0. Transport     TCP, QUIC, etc.        │
└──────────────────────────────────────────┘
```

The content store isn't a fifth layer — it's a service that Layer 3 uses. Objects reference blobs. The content store resolves those references to bytes.

## Mutable State vs. Immutable Assets

Important distinction. Not everything is a blob.

| | Mutable state | Immutable asset |
|-|---------------|-----------------|
| **Example** | HP=50, position=(3,7) | 3D mesh, texture, audio |
| **Changes?** | Yes, via Transforms | Never (new content = new blob) |
| **Where?** | Hosting domain, in memory | Content store, fetched on demand |
| **Size** | Small (usually) | Anything |
| **Replication** | Only to authorized parties | Anyone with the hash |

An object might have both: mutable state (managed through the capability protocol) and references to immutable assets (resolved through the content store). The sword's HP is mutable state. The sword's 3D model is an immutable blob.

## What This Doesn't Cover

- **Blob discovery across unconnected endpoints.** If A has a blob and B wants it, but A and B have no session, how does B find A? Gossip discovery (see [discovery.md](discovery.md)) helps — B can learn about A through the peer table. But content-specific routing (who has which blob) is a layer above basic peer discovery.
- **Encryption at rest.** The content store deals with bytes. Encrypting them before storage is application policy.
- **Garbage collection.** When no reference points to a blob, it can be cleaned up. Distributed GC is already an open problem in the protocol spec — content blobs inherit that problem.
- **Streaming.** Live audio/video is not content-addressed (it doesn't exist yet). That's a different protocol concern.

## Open Questions

- **Hash algorithm.** SHA-256 is the default. BLAKE3 is faster. Does performance matter enough at the protocol level, or is this a transport optimization?
- **Chunk size.** 256KB is a guess. Too small = too many chunks = overhead. Too large = poor dedup, no partial fetch benefit. Needs benchmarking.
- **Manifest format.** How does the chunk list / Merkle tree get serialized? This is wire format, tied to the serialization decision in the protocol spec.
- **Cross-endpoint cache coordination.** Should endpoints advertise what blobs they have? Or is "just ask and see" good enough? Advertising scales poorly. Asking has latency.
- **Content types.** Should the content store know about types (image, mesh, script), or is everything opaque bytes with type info in the object reference? Leaning opaque — keep the store simple.
