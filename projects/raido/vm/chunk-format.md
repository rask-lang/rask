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
