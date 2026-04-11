<!-- id: raido.chunk -->
<!-- status: proposed -->
<!-- summary: Compiled chunk format -- bytecode, type metadata, imports/exports, validation, content identity -->
<!-- depends: raido/vm/architecture.md -->

# Chunk Format

A compiled chunk is the output of `vm.compile()`. Contains bytecode, type metadata, and enough structure for validation and trust.

## Format

```
+---------------------+
| header              |  magic bytes, format version
| content_hash        |  SHA-256 of bytecode + constants + prototypes + types
+---------------------+
| type_table          |  struct layouts, enum definitions, extern declarations
| imports[]           |  extern funcs + extern structs this script needs
| module_imports[]    |  content-addressed module dependencies
| exports[]           |  top-level functions the host can call
+---------------------+
| constants           |  constant pool (strings, numbers)
| prototypes[]        |  function prototypes (register count, arity, param types, return type)
| bytecode            |  instructions
+---------------------+
| debug (optional)    |  source locations, function names
+---------------------+
```

## Type Table

Static types require metadata in the chunk so the VM can:
- Allocate structs with the correct layout (field count, field sizes)
- Validate extern struct bindings at load time
- Deserialize registers with the correct types (no per-value tags)

The type table contains:
- **Struct definitions.** Name, field names, field types, field count. Layout order matches declaration order.
- **Enum definitions.** Name, variant names, variant payloads (if any).
- **Extern struct declarations.** Name, field names, field types, readonly flags.
- **Extern func declarations.** Name, parameter types, return type.

The type table is part of the content hash -- changing a struct definition changes the chunk's identity.

## Content Identity

SHA-256 of the bytecode + constants + prototypes + type table sections. Computed at compile time, stored in the chunk header.

The host uses this however it wants -- comparing against expected hashes, including in audit logs, verifying snapshots came from a known script. The VM computes the hash and exposes it. The VM does not own the trust model.

```rask
const chunk = try vm.compile("script.raido", source)
const hash = chunk.hash()  // [u8; 32] -- SHA-256 of bytecode content
```

Serialized snapshots include the bytecode hash so the host can verify "this snapshot came from this script" on restore.

## Imports and Exports

**Imports** -- extern structs and extern funcs the script declares. Derived from `extern struct` and `extern func` declarations. Checked at load time: if the host hasn't bound a required extern, `vm.load()` fails immediately with a clear error listing missing bindings.

**Module imports** -- other content-addressed chunks referenced via `import "name" as alias`. The import graph is part of the chunk's content hash. The host resolves import names to chunks.

**Exports** -- top-level `func` declarations with their typed signatures. The host can inspect what's callable and what types are expected without reading source.

```rask
const chunk = try vm.compile("script.raido", source)
chunk.imports()         // extern declarations needed
chunk.module_imports()  // ["combat_utils", "physics_common"]
chunk.exports()         // [("on_update", ...), ("process", ...)]

// Load fails fast if externs aren't satisfied
vm.register_extern_struct("Enemy", ...)
vm.register_extern_func("move_to", move_to_handler)
try vm.load(chunk)  // fails if any extern unbound
```

## Validation

Single-pass structural validation runs during `vm.compile()` (for source) or `vm.load()` (for pre-compiled bytecode). Rejects malformed chunks before execution.

Checks:
- Register indices within frame size
- Jump targets within bytecode bounds
- Constant pool indices valid
- Prototype indices valid (for `FUNC_REF`, `NEW_STRUCT`)
- Struct field indices within field count
- Extern struct/func declarations match host bindings (types, field count, readonly flags)
- Format version recognized
- Type table consistency (no undefined type references)

`vm.load(bytes)` validates pre-compiled bytecode from disk or network. If validation fails, load returns an error -- no partial execution, no undefined behavior.

```rask
// Compile from source (validates during compilation)
const chunk = try vm.compile("script.raido", source)

// Load pre-compiled bytecode (validates on load)
const chunk = try vm.load(bytecode_bytes)
```

## Version Compatibility

The format version in the chunk header is an integer, starting at 1. It determines bytecode encoding, constant pool layout, type table format, and instruction semantics. Two VMs can re-execute each other's chunks only if they agree on the format version.

### Compatibility Matrix

| Relationship | Compatible? | Notes |
|-------------|-------------|-------|
| Same version | Yes | Bitwise-identical execution guaranteed |
| Newer VM, older chunk | Yes, with constraints | VM must include the older version's instruction semantics. Execution uses the chunk's declared version, not the VM's latest. |
| Older VM, newer chunk | No | `vm.load()` returns `VersionMismatch`. The VM cannot execute instructions it doesn't understand. |

A VM that supports versions 1--3 can load and execute a v1 chunk using v1 semantics. It cannot "upgrade" the chunk -- it runs it as-is. This is what makes cross-domain verification work: both sides execute the same bytecode with the same version's rules.

### Version Metadata in Chunks

The chunk header carries:

| Field | Type | Purpose |
|-------|------|---------|
| `magic` | `[u8; 4]` | Format identifier (`RADO`) |
| `version` | `u16` | Chunk format version |
| `content_hash` | `[u8; 32]` | SHA-256 of bytecode + constants + prototypes + types |

The version field is checked first during `vm.load()`. If the VM doesn't support that version, it rejects immediately -- before reading any other section.

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

Both domains compute the intersection of supported versions. Cross-domain Proofs for scripted transforms must use a chunk format version both sides support. If the intersection is empty, verifiable transforms are unavailable for this session -- the domains fall back to trust-based verification.

### What Happens: Version Mismatch Scenarios

**Domain A has a v2 script, Domain B only runs v1.**

B cannot verify A's v2 transform. Three options, in order of preference:

1. **A provides a v1-compatible script.** If the logic can be expressed in v1, A maintains both versions. The v1 script has a different content hash -- both hashes are registered as equivalent minting authorities.
2. **B upgrades.** B deploys a VM that supports v2.
3. **Fall back to trust-based.** B accepts A's Proof structurally but cannot mechanically verify the computation. Unverified transforms carry lower weight in reputation scoring.

No silent degradation. B always knows whether it verified mechanically or accepted on trust.

### Script Migration

A chunk's content hash is its identity. Changing bytecode changes the hash. Migration must be provable -- any domain can independently reproduce the translation and verify the output hash.

**When migration works:** version bumps that change encoding or instruction layout but not semantics. A mechanical `v1 -> v2` translation is a deterministic function of the input bytecode.

**When it doesn't:** version bumps that change instruction semantics. The old script stays at v1; domains that need to verify it must support v1.

**Equivalence registration.** A domain that migrates a script publishes both hashes (old and new) as equivalent minting authorities. Verifying domains confirm equivalence by running the migration themselves.

Concrete migration tooling is deferred until the first version bump.

### Serialization Compatibility

VM serialization (snapshots) has its own version header, separate from the chunk format version. A snapshot includes:

| Field | Purpose |
|-------|---------|
| `serialization_version` | Snapshot format version |
| `chunk_version` | The chunk format version of the bytecode being executed |
| `chunk_hash` | Content hash of the bytecode (for re-loading) |

Deserialize rejects unknown serialization versions with `VersionMismatch`. The chunk format version in the snapshot tells the restoring VM which instruction semantics to use.

## Debug Sections

Optional. Stripped by default in release, included with a compile flag.

- **Source locations.** Maps bytecode offsets to source file + line. Powers stack traces in `raido.ScriptError`.
- **Function names.** Maps prototype indices to names. Powers readable stack traces.

```rask
const chunk = try vm.compile("script.raido", source, raido.CompileOpts {
    debug_info: true,
})
```

When debug info is absent, stack traces show `<chunk>:proto[3]:offset[42]` instead of `script.raido:27 in process()`.
