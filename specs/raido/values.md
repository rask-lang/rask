<!-- id: raido.values -->
<!-- status: proposed -->
<!-- summary: Raido value model — tagged values, deterministic softfloat, serializable -->
<!-- depends: raido/README.md, memory/pools.md -->

# Values

All values are 8 bytes. Must be fully serializable — no pointers to host memory, no platform-dependent representation.

## Types

| Type | Description |
|------|-------------|
| `nil` | Absence. Falsy. |
| `bool` | `true`/`false`. Only `nil` and `false` are falsy. |
| `int` | i64. Entity IDs, counters, bitfields. |
| `number` | Softfloat f64. Deterministic across platforms. |
| `string` | Immutable, UTF-8, arena-allocated. Copied at host boundary. |
| `array` | 0-indexed growable sequence. `[a, b, c]`. |
| `map` | Unordered key-value store. `{k: v}`. Keys: string, int, bool. |
| `function` | Closure (bytecode index + captured upvalues) or host function name. |
| `handle` | `Handle<T>` from Rask pools. `h.field` does a pool lookup. |
| `userdata` | Opaque serializable blob. No `@resource` types. |

## Representation

NaN-boxing or tagged union — implementation choice. The constraint is serializability: every value must round-trip through `serialize`/`deserialize` with bitwise identity. No raw pointers in the value representation — arena references use offsets, not addresses.

## Number (Softfloat)

- IEEE 754 f64 semantics, software-emulated.
- Bitwise-identical results on all platforms.
- `42` is int, `42.0` is number. `int + int → int`, `int + number → number`.
- `/` always returns number.
- `42 == 42.0` is `true`.
- Int overflow wraps.

The `number` type stores the same bits as an f64, but all arithmetic goes through softfloat routines instead of hardware FPU instructions. This is the determinism guarantee.

## Strings

Arena-allocated. Copied at the Rask/Raido boundary. Interned literals for fast equality. Serialized as length + bytes.

## Arrays and Maps

Separate types. `[1, 2, 3]` is an array, `{x: 1}` is a map. Serialized as length + elements.

Map key types restricted to string, int, bool — types with stable, deterministic equality and hashing. No `number` keys (float equality is well-defined but confusing). No array/map/function keys.

## Handles

Stored as pool-name + generation + index. Serializable as-is — handles are just integers. Field access resolves through the pool at runtime, not stored in the handle.

## Functions

Script functions stored as bytecode chunk index + upvalue list. Host functions stored as string names. On deserialize, host function names are re-resolved against the registered function table.

This means closures are serializable — they're a bytecode reference + captured values, not a pointer to native code.

## Userdata

Must be serializable. The host registers a serialize/deserialize pair for each userdata type. Non-serializable Rask types (anything with pointers, handles, resources) cannot be userdata.

This is more restrictive than the previous design. If a userdata type can't round-trip through bytes, it can't live in the VM.
