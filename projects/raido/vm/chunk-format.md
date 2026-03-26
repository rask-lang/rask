<!-- id: raido.chunk -->
<!-- status: proposed -->
<!-- summary: Compiled chunk format — bytecode, imports/exports, validation, content identity -->
<!-- depends: raido/vm/architecture.md -->

# Chunk Format

A compiled chunk is the output of `vm.compile()`. Contains bytecode, metadata, and enough structure for validation and trust.

## Format

```
┌─────────────────────┐
│ header              │  magic bytes, format version
│ content_hash        │  SHA-256 of bytecode + constants + prototypes
├─────────────────────┤
│ imports[]           │  host functions this script calls
│ exports[]           │  top-level functions the host can call
├─────────────────────┤
│ constants           │  constant pool (strings, numbers)
│ prototypes[]        │  function prototypes (register count, upvalue count, arity)
│ bytecode            │  instructions
├─────────────────────┤
│ debug (optional)    │  source locations, function names
└─────────────────────┘
```

## Content Identity

SHA-256 of the bytecode + constants + prototypes sections. Computed at compile time, stored in the chunk header.

The host uses this however it wants — comparing against expected hashes, including in audit logs, verifying snapshots came from a known script. The VM computes the hash and exposes it. The VM does not own the trust model.

```rask
const chunk = try vm.compile("script.raido", source)
const hash = chunk.hash()  // [u8; 32] — SHA-256 of bytecode content
```

Serialized snapshots include the bytecode hash so the host can verify "this snapshot came from this script" on restore.

## Imports and Exports

**Auto-derived.** The compiler builds these from the source — no annotations, no declarations in the script language.

**Imports** — host functions the script calls. The compiler scans call sites, any function name that isn't locally defined or a stdlib built-in is an import. Checked at load time: if the host hasn't registered a required import, `vm.exec()` fails immediately with a clear error listing missing functions.

```rask
const chunk = try vm.compile("script.raido", source)
chunk.imports()   // ["send_message", "spawn_enemy", "play_sound"]
chunk.exports()   // ["on_update", "on_damage", "process"]

// Load fails fast if imports aren't satisfied
vm.register("send_message", |ctx| { ... })
// vm.exec(chunk) would fail: missing "spawn_enemy", "play_sound"
```

**Exports** — top-level `func` declarations. The host can inspect what's callable without reading source.

**Script authors don't see any of this.** They write functions and call host functions by name. The compiler does the bookkeeping.

## Validation

Single-pass structural validation runs during `vm.compile()` (for source) or `vm.load()` (for pre-compiled bytecode). Rejects malformed chunks before execution.

Checks:
- Register indices within frame size
- Jump targets within bytecode bounds
- Constant pool indices valid
- Upvalue indices within prototype's upvalue count
- Prototype arity matches call sites where inferrable
- Format version recognized

`vm.load(bytes)` validates pre-compiled bytecode from disk or network. If validation fails, load returns an error — no partial execution, no undefined behavior.

```rask
// Compile from source (validates during compilation)
const chunk = try vm.compile("script.raido", source)

// Load pre-compiled bytecode (validates on load)
const chunk = try vm.load(bytecode_bytes)
```

## Version Compatibility

The format version in the chunk header is an integer, starting at 1. It determines bytecode encoding, constant pool layout, and instruction semantics. Two VMs can re-execute each other's chunks only if they agree on the format version.

### Compatibility Matrix

| Relationship | Compatible? | Notes |
|-------------|-------------|-------|
| Same version | Yes | Bitwise-identical execution guaranteed |
| Newer VM, older chunk | Yes, with constraints | VM must include the older version's instruction semantics. Execution uses the chunk's declared version, not the VM's latest. |
| Older VM, newer chunk | No | `vm.load()` returns `VersionMismatch`. The VM cannot execute instructions it doesn't understand. |

A VM that supports versions 1–3 can load and execute a v1 chunk using v1 semantics. It cannot "upgrade" the chunk — it runs it as-is. This is what makes cross-domain verification work: both sides execute the same bytecode with the same version's rules.

### Version Metadata in Chunks

The chunk header carries:

| Field | Type | Purpose |
|-------|------|---------|
| `magic` | `[u8; 4]` | Format identifier (`RADO`) |
| `version` | `u16` | Chunk format version |
| `content_hash` | `[u8; 32]` | SHA-256 of bytecode + constants + prototypes |

The version field is checked first during `vm.load()`. If the VM doesn't support that version, it rejects immediately — before reading any other section. This prevents misinterpreting bytecode encoded under unknown rules.

### Cross-Domain Version Agreement

When two domains negotiate verifiable transforms over [Leden](../../leden/), they include Raido version support in the capability negotiation:

```
Hello(min=1, max=1, ext=[content_store, verifiable_transform])
Welcome(version=1, ext=[content_store, verifiable_transform])
```

The `verifiable_transform` extension carries additional parameters during negotiation:

| Parameter | Type | Purpose |
|-----------|------|---------|
| `raido_versions` | `array<uint>` | Chunk format versions this domain can execute |
| `raido_serialization_version` | `uint` | VM serialization format version (for snapshot exchange) |

Both domains compute the intersection of supported versions. Cross-domain Proofs for scripted transforms must use a chunk format version both sides support. If the intersection is empty, verifiable transforms are unavailable for this session — the domains fall back to trust-based verification (Proof structure only, no re-execution).

### What Happens: Version Mismatch Scenarios

**Domain A has a v2 script, Domain B only runs v1.**

B cannot verify A's v2 transform. Three options, in order of preference:

1. **A provides a v1-compatible script.** If the minting logic can be expressed in v1, A maintains both versions. The v1 script has a different content hash — both hashes are registered in A's supply audit as equivalent minting authorities for the same asset type.
2. **B upgrades.** B deploys a VM that supports v2. This is a software update, not a protocol change.
3. **Fall back to trust-based.** B accepts A's Proof structurally but cannot mechanically verify the computation. B's trust model accounts for this — unverified transforms carry lower weight in reputation scoring. This is the default when `verifiable_transform` negotiation fails.

No silent degradation. B always knows whether it verified mechanically or accepted on trust. The distinction is recorded in B's local audit log.

**A domain upgrades its minting script from v1 to v2.**

The old v1 script's content hash remains valid for historical audits. Supply audit entries reference the script hash that produced them — a v1 mint stays linked to the v1 script, a v2 mint to the v2 script. Verifying domains fetch the script version that matches each audit entry's hash.

### Script Migration

A chunk's content hash is its identity. Changing bytecode changes the hash, which breaks audit references. So migration must be provable — any domain can independently reproduce the translation and verify the output hash.

**When migration works:** version bumps that change encoding or instruction layout but not semantics. A mechanical `v1 → v2` translation is a deterministic function of the input bytecode. Any party can run it and confirm the output matches the claimed new hash.

**When it doesn't:** version bumps that change instruction semantics. If v2 redefines what an instruction means, there's no mechanical translation. The old script stays at v1; domains that need to verify it must support v1.

**Equivalence registration.** A domain that migrates a script publishes both hashes (old and new) as equivalent minting authorities for the same asset type. Verifying domains confirm equivalence by running the migration themselves. No trust required.

Concrete migration tooling is deferred until the first version bump — the mechanism depends on what actually changes between versions.

### Serialization Compatibility

VM serialization (snapshots) has its own version header, separate from the chunk format version. A snapshot includes:

| Field | Purpose |
|-------|---------|
| `serialization_version` | Snapshot format version |
| `chunk_version` | The chunk format version of the bytecode being executed |
| `chunk_hash` | Content hash of the bytecode (for re-loading) |

Deserialize rejects unknown serialization versions with `VersionMismatch`. The chunk format version in the snapshot tells the restoring VM which instruction semantics to use — the VM must support that chunk version to resume execution.

**Forward compatibility:** New serialization versions may add fields. Old VMs reject them (unknown version). No attempt to skip unknown fields — the snapshot format is not self-describing.

**Backward compatibility:** A newer VM can deserialize older snapshots if it retains the older serialization logic. Each serialization version is a distinct code path, not a layered extension. This keeps deserialization simple and auditable — no accumulated migration transforms.

**Policy:** Serialization versions are supported for as long as any actively-traded scripts might have snapshots in that format. In practice, the VM ships with support for the current version and the previous one. Domains that need longer support pin their VM version.

## Debug Sections

Optional. Stripped by default in release, included with a compile flag.

- **Source locations.** Maps bytecode offsets to source file + line. Powers stack traces in `raido.ScriptError`.
- **Function names.** Maps prototype indices to names. Powers readable stack traces.

```rask
const chunk = try vm.compile("script.raido", source, raido.CompileOpts {
    debug_info: true,
})
```

When debug info is absent, stack traces show `<chunk>:proto[3]:offset[42]` instead of `script.raido:27 in process()`. Functional but less readable.
