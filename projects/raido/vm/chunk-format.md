<!-- id: raido.chunk -->
<!-- status: proposed -->
<!-- summary: Compiled chunk format -- bytecode, type metadata, imports/exports, validation, content identity -->
<!-- depends: raido/vm/architecture.md -->

# Chunk Format

A compiled chunk is the output of `raido.compile()`. Contains bytecode, type metadata, and enough structure for validation and trust.

## Format

```
+---------------------+
| header              |  magic bytes, format version, profile
| content_hash        |  SHA-256 of everything below that carries meaning
+---------------------+
| type_table          |  struct, tuple and enum layouts, extern declarations
| variants[]          |  (enum type, variant) pairs for NEW_ENUM
| api_hash            |  content hash of the published API module
| imports[]           |  extern funcs + extern structs this script uses
| builtins[]          |  the built-ins it calls, by name, for CALL_BUILTIN
| module_imports[]    |  content-addressed module dependencies (name + hash)
| functions[]         |  own prototypes, then imported functions (module, name)
| module_vars[]       |  own top-level `let`s (name, type), then imported ones (module, name)
| init                |  the prototype that runs the initializers, in declaration order
| exports[]           |  top-level functions the host can call
+---------------------+
| constants           |  ints, floats and strings
| prototypes[]        |  register types, arity, capture count, return type, bytecode
+---------------------+
| debug (optional)    |  source locations, function names
+---------------------+
```

## Type Table

Static types require metadata in the chunk so the VM can:
- Allocate structs with the correct layout (field count, field sizes)
- Validate extern struct bindings at load time
- Know each container's element type, and which element types keep their tag

The type table contains:
- **Struct definitions.** Name, field names, field types, field count. Layout order matches declaration order.
- **Enum definitions.** Name, variant names, variant payloads (if any).
- **Extend methods.** Type name, method name, parameter types, return type, prototype index. Methods are compiled as regular functions — extend is resolved at compile time, not runtime.
- **Extern struct declarations.** Name, field names, field types, readonly flags.
- **Extern func declarations.** Name, parameter types, return type, and whether it's immediate, deferred or pausing.

The type table is part of the content hash -- changing a struct definition changes the chunk's identity.

Every index an instruction carries points into one of these tables, so bytecode never names anything by string at run time. Imported functions and module variables, and the built-ins, are named in their tables and resolved once at load: a built-in name the VM doesn't know, or an import that doesn't export the name, fails the load. A built-in table by name, rather than a global numbering, means adding a built-in later doesn't renumber anyone's bytecode.

Each prototype lists a type for every register. A register holds that type for the whole function, which is what lets the verifier check each instruction on its own ([architecture.md](architecture.md#verification)).

## Content Identity

SHA-256 over everything that decides what the chunk does: the format version, the profile, the type table, the API module's hash, the module imports (names and resolved hashes), module variables, exports, constants, prototypes and bytecode. Debug sections are left out. Computed at compile time, stored in the chunk header.

Leaving any of these out would let one hash run under two meanings: the same bytecode against a different API, under a different profile's memory rule, or with a different imported module.

The host uses this however it wants -- comparing against expected hashes, including in audit logs, verifying snapshots came from a known script. The VM computes the hash and exposes it. The VM does not own the trust model.

```rask
let chunk = try raido.compile("script.rd", source, api, Vec.new(), raido.CompileOpts.default())
let hash = chunk.hash()  // 32 bytes
```

Serialized snapshots include the bytecode hash so the host can verify "this snapshot came from this script" on restore.

## Imports and Exports

**Imports** -- the extern structs and extern funcs the script uses from the published API module, and that module's hash. Checked at load time: if the host's bindings don't match the module, or it hasn't bound something the script uses, `vm.load()` fails with a clear error listing what's missing.

**Module imports** -- other content-addressed chunks referenced via `import "name" as alias`. The import graph is part of the chunk's content hash. The host resolves import names to chunks.

**Exports** -- top-level `func` declarations with their typed signatures. The host can inspect what's callable and what types are expected without reading source.

```rask
chunk.exports()         // name, parameter types and result type of each
try vm.load(chunk, library)  // fails if the bindings leave out anything the API needs
```

## Validation

`raido.compile()` checks source; `raido.Chunk.decode(bytes)` checks bytecode from disk or network, recomputes the hash, and verifies every function ([architecture.md](architecture.md#verification)). Beyond the per-instruction checks there:

- the format version is recognized;
- the type table is consistent: no undefined type references;
- every table index is in range;
- a pure or provable chunk keeps its profile's rules, and a provable one contains no float constants or float instructions ([architecture.md](architecture.md#profiles)).

Extern declarations are checked against the host's bindings at `vm.load()`. If any check fails, nothing runs.

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
| `profile` | `u8` | 0 = normal, 1 = pure, 2 = provable |
| `content_hash` | `[u8; 32]` | SHA-256 over the sections listed in [Content Identity](#content-identity) |

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
let chunk = try raido.compile("script.rd", source, api, Vec.new(), raido.CompileOpts(debug_info: true, profile: raido.Profile.Normal))
```

When debug info is absent, stack traces show `<chunk>:proto[3]:offset[42]` instead of `script.rd:27 in process()`.
