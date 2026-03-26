<!-- id: leden.wire-format -->
<!-- status: proposed -->
<!-- summary: MessagePack wire format, message encoding, schema evolution -->

# Wire Format

<!-- Decision: MessagePack with integer-keyed maps. Self-describing, compact, ~50 language implementations. No IDL conflict with Leden's own type model. -->

MessagePack encoding for all Leden protocol messages. Integer-keyed fields for compactness with schema evolution support.

## Framing

Every message on the wire is length-prefixed:

```
┌──────────────┬────────────────────────┐
│ length: u32  │ payload: msgpack bytes │
└──────────────┘────────────────────────┘
```

- `length` is 4 bytes, big-endian, excluding itself. Maximum message size: 16 MiB (enforced, not implied by u32).
- `payload` is a single MessagePack value — always a map with integer keys.

Big-endian for the length prefix because network byte order. The rest is MessagePack's own encoding.

## Message Envelope

Every message is a MessagePack map with integer keys:

| Field ID | Name | Type | Required | Purpose |
|----------|------|------|----------|---------|
| 0 | `type` | uint | yes | Message type tag |
| 1 | `id` | uint | conditional | Request ID for request/response correlation |
| 2+ | | varies | | Message-specific fields |

Field 1 (`id`) is present on all request/response messages. Fire-and-forget messages (like `RevocationNotice`) omit it.

### Message Type Tags

Core protocol:

| Tag | Message |
|-----|---------|
| 0x01 | Hello |
| 0x02 | Welcome |
| 0x03 | Incompatible |
| 0x04 | Bootstrap |
| 0x05 | BootstrapResult |
| 0x06 | Introduce |
| 0x07 | Reattach |
| 0x08 | ReattachResult |
| 0x10 | Call |
| 0x11 | Return |
| 0x12 | Error |

Capability lifecycle:

| Tag | Message |
|-----|---------|
| 0x20 | Revoke |
| 0x21 | RevocationNotice |
| 0x22 | RevocationAck |
| 0x23 | CheckRevocation |
| 0x24 | RevocationStatus |
| 0x25 | Release |
| 0x26 | Renew |
| 0x27 | LeaseExpired |
| 0x28 | GCProbe |

Discovery extension (0x40–0x4F):

| Tag | Message |
|-----|---------|
| 0x40 | PeerDigest |
| 0x41 | PeerRequest |
| 0x42 | PeerUpdate |

Observation extension (0x50–0x5F):

| Tag | Message |
|-----|---------|
| 0x50 | Observe |
| 0x51 | Update |
| 0x52 | Unobserve |
| 0x53 | ObserveBatch |
| 0x54 | UnobserveBatch |

Content store extension (0x60–0x6F):

| Tag | Message |
|-----|---------|
| 0x60 | ContentRequest |
| 0x61 | ContentResponse |

Tags 0x80–0xFF are reserved for application-defined messages.

Gaps between ranges are intentional — room for future core messages without colliding with extensions.

---

## Format Version Negotiation

The wire format has its own version, separate from the protocol version. This keeps encoding concerns decoupled — a format optimization doesn't force a protocol version bump.

Format version is a single integer, starting at 1. It's carried in the Hello/Welcome handshake as one additional field. No min/max range — the client states what format version it's sending, the server either accepts or rejects.

The Hello message is always encoded using format version 1 rules. This bootstraps the negotiation — both sides must understand format v1 to handshake, even if they'll use a newer format for everything after.

```
Client                                Server
   |                                     |
   |  Hello(format=2, min=1, max=3,     |
   |        ext=[...])                   |
   |────────────────────────────────────>|
   |                                     |
   |  Welcome(format=2, version=3,      |
   |          ext=[...])                 |
   |<────────────────────────────────────|
   |                                     |
   |  (all subsequent messages use       |
   |   format version 2)                 |
```

If the server doesn't support the client's format version, it responds with `Incompatible` (which now also carries the server's supported format versions). The client can reconnect with a different format version.

---

## Message Schemas

Each message is a MessagePack map. Field IDs are permanent — once assigned, never reused. Fields not listed are ignored by the decoder (forward compatibility).

### Types used in schemas

| Notation | MessagePack type |
|----------|-----------------|
| `uint` | positive integer |
| `int` | integer (signed) |
| `bytes` | bin |
| `string` | str |
| `bool` | boolean |
| `array<T>` | array of T |
| `map<K,V>` | map with K keys and V values |
| `optional` | field may be absent |

### Hello (0x01)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x01` |
| 2 | format | uint | Wire format version the client wants to use |
| 3 | min | uint | Minimum protocol version supported |
| 4 | max | uint | Maximum protocol version supported |
| 5 | ext | array\<string\> | Supported extensions |

### Welcome (0x02)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x02` |
| 2 | format | uint | Accepted wire format version |
| 3 | version | uint | Negotiated protocol version |
| 4 | ext | array\<string\> | Accepted extensions (intersection) |

### Incompatible (0x03)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x03` |
| 2 | server_min | uint | Server's minimum protocol version |
| 3 | server_max | uint | Server's maximum protocol version |
| 4 | formats | array\<uint\> | Wire format versions the server supports |

### Bootstrap (0x04)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x04` |
| 1 | id | uint | Request ID |
| 2 | credentials | bytes | Application-defined proof of identity |

### BootstrapResult (0x05)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x05` |
| 1 | id | uint | Request ID (matches Bootstrap) |
| 2 | greeter | bytes | Encoded capability reference |

### Introduce (0x06)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x06` |
| 1 | id | uint | Request ID |
| 2 | capability | bytes | The capability being delegated |
| 3 | recipient | bytes | Endpoint identity of the intended recipient |
| 4 | attenuation | optional uint | Permission bitfield for the narrowed capability |

### Reattach (0x07)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x07` |
| 1 | id | uint | Request ID |
| 2 | sturdy_refs | array\<bytes\> | Encoded sturdy references to recover |

### ReattachResult (0x08)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x08` |
| 1 | id | uint | Request ID (matches Reattach) |
| 2 | results | array\<map\> | Per-ref result (see below) |

Each entry in `results`:

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | capability | optional bytes | Live capability, if successful |
| 1 | error | optional uint | Error code, if failed |

### Sturdy Reference Encoding

Each sturdy reference in the `sturdy_refs` array is a MessagePack map:

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | issuer | bytes | Endpoint identity of the original issuer |
| 1 | object_id | bytes | Object this capability grants access to |
| 2 | permissions | uint | Permission bitfield (after attenuation) |
| 3 | nonce | bin(32) | 256-bit nonce proving issuance |
| 4 | delegation_chain | array\<bin(32)\> | HMAC-SHA256 link hashes, one per delegation step |
| 5 | expiry | optional uint | Unix timestamp (seconds), absent if no expiry |

The `delegation_chain` array has at least one entry (the root link). Maximum length is implementation-defined (recommended: 32).

### Call (0x10)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x10` |
| 1 | id | uint | Request ID |
| 2 | target | bytes | Object reference or promise reference |
| 3 | method | string | Method name |
| 4 | args | bytes | MessagePack-encoded arguments |
| 5 | pipeline | optional array | Pipelined call chain (see below) |

Each pipeline entry:

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | method | string | Method to call on the result |
| 1 | args | bytes | MessagePack-encoded arguments |

### Return (0x11)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x11` |
| 1 | id | uint | Request ID (matches Call) |
| 2 | value | bytes | MessagePack-encoded return value |
| 3 | caps | optional array\<bytes\> | Capability references included in the return value |

### Error (0x12)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x12` |
| 1 | id | uint | Request ID (matches Call) |
| 2 | code | uint | Error code (see protocol.md error codes) |
| 3 | message | string | Human-readable description |
| 4 | data | optional bytes | Structured error detail |

### Revoke (0x20)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x20` |
| 1 | id | uint | Request ID |
| 2 | capability_id | bytes | Capability to revoke |

### RevocationNotice (0x21)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x21` |
| 2 | capability_id | bytes | Revoked capability |
| 3 | reason | optional string | Why it was revoked |

No `id` field — this is a notification, not a request.

### RevocationAck (0x22)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x22` |
| 2 | capability_id | bytes | Acknowledged capability |

### CheckRevocation (0x23)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x23` |
| 1 | id | uint | Request ID |
| 2 | capability_id | bytes | Capability to check |

### RevocationStatus (0x24)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x24` |
| 1 | id | uint | Request ID (matches CheckRevocation) |
| 2 | capability_id | bytes | |
| 3 | valid | bool | Still valid? |

### Release (0x25)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x25` |
| 2 | capability_id | bytes | |
| 3 | weight | uint | Weight being returned |

### Renew (0x26)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x26` |
| 1 | id | uint | Request ID |
| 2 | capability_id | bytes | |

### LeaseExpired (0x27)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x27` |
| 2 | capability_id | bytes | |

### GCProbe (0x28)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x28` |
| 2 | capability_id | bytes | |
| 3 | probe_id | bytes | Unique probe identifier for cycle detection |

### PeerDigest (0x40)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x40` |
| 2 | entries | array\<map\> | Digest entries (see below) |

Each digest entry:

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | endpoint_id | bytes | |
| 1 | generation | uint | Monotonic restart counter |
| 2 | last_seen | uint | Unix timestamp (seconds) |

### PeerRequest (0x41)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x41` |
| 2 | endpoint_ids | array\<bytes\> | Endpoints to get full entries for |

### PeerUpdate (0x42)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x42` |
| 2 | entries | array\<map\> | Full peer entries (see below) |

Each peer entry:

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | endpoint_id | bytes | Cryptographic identity |
| 1 | addresses | array\<string\> | Transport addresses |
| 2 | last_seen | uint | Unix timestamp (seconds) |
| 3 | generation | uint | Monotonic restart counter |
| 4 | metadata | optional map\<string,string\> | Application-defined tags |

### Observe (0x50)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x50` |
| 1 | id | uint | Request ID |
| 2 | object_ref | bytes | Object or promise reference |
| 3 | credits | uint | Initial backpressure credits |
| 4 | filter | optional array\<string\> | Field names to include |

### Update (0x51)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x51` |
| 2 | observation_id | uint | Matches the request ID of the Observe |
| 3 | seq | uint | Sequence number |
| 4 | delta | optional bytes | MessagePack-encoded delta |
| 5 | snapshot | optional bytes | MessagePack-encoded full state |

Exactly one of `delta` or `snapshot` is present.

### Unobserve (0x52)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x52` |
| 2 | observation_id | uint | |

### ObserveBatch (0x53)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x53` |
| 1 | id | uint | Request ID |
| 2 | observations | array\<map\> | Per-item observe params |

Each observation entry uses the same fields as Observe (minus `type`):

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | object_ref | bytes | |
| 1 | credits | uint | |
| 2 | filter | optional array\<string\> | |

### UnobserveBatch (0x54)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x54` |
| 2 | observation_ids | array\<uint\> | |

### ContentRequest (0x60)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x60` |
| 1 | id | uint | Request ID |
| 2 | hash | bytes | Content hash |
| 3 | offset | optional uint | Byte offset for partial/resumed fetch |
| 4 | length | optional uint | Max bytes to return |

### ContentResponse (0x61)

| Field | Name | Type | Notes |
|-------|------|------|-------|
| 0 | type | uint | `0x61` |
| 1 | id | uint | Request ID (matches ContentRequest) |
| 2 | data | optional bytes | Content bytes (absent on error) |
| 3 | total_size | optional uint | Total blob size (for progress tracking) |
| 4 | error | optional uint | Error code if fetch failed |

---

## Schema Evolution Rules

These are the rules for evolving message schemas across protocol versions without breaking existing implementations.

### Adding fields

Add new fields with unused field IDs. The new field must be optional or have a well-defined default. Decoders that don't know the field skip it.

### Removing fields

Stop sending the field. Don't reuse the field ID — ever. Old decoders that expect the field must already handle its absence (because it was optional or had a default).

### Changing field types

Don't. Add a new field with the new type, deprecate the old one. Both may coexist during a transition period.

### Rules summary

1. **Field IDs are permanent.** Once assigned, a field ID is bound to that name and type forever within its message type.
2. **New fields must be optional.** A decoder from an older version must be able to decode the message without the new field.
3. **Unknown fields are ignored.** A decoder must skip field IDs it doesn't recognize. This is the core forward-compatibility mechanism.
4. **Unknown message types are ignored.** If a decoder encounters a message type tag it doesn't recognize, it discards the message and logs it. No error response — the sender is using a newer protocol version with new message types.
5. **Required fields can never become optional.** That would break old encoders that omit them.
6. **Optional fields can become required** only in a new major protocol version.
7. **No reordering dependency.** Decoders must not assume field order in the map. MessagePack maps have no guaranteed order.

### Versioning interaction

- **Minor protocol version bump**: new optional fields, new message types, new extension message types. All backward-compatible under the rules above.
- **Major protocol version bump**: required field changes, removed message types, semantic changes to existing fields.
- **Format version bump**: changes to framing, envelope structure, or encoding conventions (e.g., switching from int-keyed maps to a more compact array encoding). Rare.

---

## Error Codes on the Wire

Error codes from protocol.md, encoded as uint:

| Value | Code |
|-------|------|
| 1 | CapabilityRevoked |
| 2 | CapabilityExpired |
| 3 | PermissionDenied |
| 4 | ObjectNotFound |
| 5 | MethodNotFound |
| 6 | RateLimited |
| 7 | EndpointUnavailable |
| 8 | Timeout |
| 9 | VersionMismatch |
| 10 | MalformedMessage |
| 11 | InvalidNonce |
| 12 | InvalidChain |
| 13 | IssuerMismatch |
| 0x1000+ | Application(n - 0x1000) |

Application error codes start at 0x1000 to leave room for future protocol-defined codes.
