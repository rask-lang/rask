<!-- id: compiler.mangling -->
<!-- status: decided -->
<!-- summary: Symbol naming scheme for object file emission -->
<!-- depends: struct.modules, type.generics -->

# Name Mangling

Symbol naming rules for object file generation. Predictable, collision-free, demangleable.

## Mangling Scheme

| Rule | Description |
|------|-------------|
| **M1: Prefix** | All Rask symbols start with `_R` |
| **M2: Encoding** | Package path, item type, name, generic args, hash |
| **M3: Deterministic** | Same source = same symbol (reproducible builds) |
| **M4: No user collision** | User code can't create symbols starting with `_R` |

## Symbol Structure

```
_R<len><pkg_path>_<type><len><name>[_G<generic_args>][_H<hash>]
```

| Component | Description | Example |
|-----------|-------------|---------|
| `_R` | Rask prefix | `_R` |
| `<len>` | Length of next segment | `4` |
| `<pkg_path>` | Dot-separated package path | `core` |
| `_` | Separator | `_` |
| `<type>` | Item type marker | `F` (function) |
| `<len>` | Length of item name | `3` |
| `<name>` | Item name | `add` |
| `_G<args>` | Generic instantiation (optional) | `_Gi32i32` |
| `_H<hash>` | Collision hash (if needed) | `_H4a3f` |

## Item Type Markers

| Type | Marker | Example |
|------|--------|---------|
| Function | `F` | `_R4core_F3add` |
| Method | `M` | `_R4core_M3Vec4push_Gi32` |
| Struct | `S` | `_R4core_S3Vec_Gi32` |
| Enum | `E` | `_R4core_E6Option_Gi32` |
| Trait | `T` | `_R4core_T5Clone` |
| Const | `C` | `_R4core_C3MAX` |
| Static | `V` | `_R4core_V5CACHE` |
| Test | `Test` | `_R4core_Test9parse_url` |
| Benchmark | `Bench` | `_R4core_Bench6decode` |
| Closure | `L` | `_R4main_L0_H3a2f` |

## Package Paths

| Rule | Description |
|------|-------------|
| **P1: Length prefix** | Each segment prefixed with decimal length |
| **P2: No dots** | Replace `.` with length boundaries |
| **P3: Nested packages** | `myapp.net.http` → `5myapp3net4http` |

```
core            → 4core
myapp.net.http  → 5myapp3net4http
```

## Generic Arguments

| Rule | Description |
|------|-------------|
| **G1: Type encoding** | Types encoded as abbreviated names |
| **G2: Nested generics** | Use brackets for nesting |
| **G3: Primitives** | Short names (i32, u64, bool, str) |

| Type | Encoding | Example |
|------|----------|---------|
| `i32` | `i32` | `_Gi32` |
| `Vec<i32>` | `Vec[i32]` | `_GVec[i32]` |
| `Map<string, User>` | `Map[string,User]` | `_GMap[string,4User]` |
| `Option<T>` | `Option[T]` | `_GOption[T]` |

User types include length prefix: `User` → `4User`, `HttpRequest` → `11HttpRequest`

## Collision Hashes

| Rule | Description |
|------|-------------|
| **H1: When needed** | Only when signature hash prevents collision |
| **H2: Short hash** | 4 hex chars from FNV-1a of full signature |
| **H3: Signature** | Hash includes all param types, return type, context clauses |

Hash needed when:
- Multiple monomorphizations would create identical symbols
- Closures in same function
- Generic specializations with complex type params

## Special Cases

### Functions

```rask
// package: myapp
public func add(a: i32, b: i32) -> i32
```
→ `_R5myapp_F3add_Gi32i32i32`

```rask
// package: core
public func sort<T>(arr: Vec<T>) using Compare<T>
```
→ `_R4core_F4sort_GVec[T]Compare[T]` (generic, not monomorphized)
→ `_R4core_F4sort_GVec[i32]Compare[i32]_H3a2f` (monomorphized for i32)

### Methods

```rask
// package: core
extend Vec<T> {
    public func push(mutate self, item: T) -> Result<(), PushError<T>>
}
```
→ `_R4core_M3Vec4push_GT` (generic method definition)
→ `_R4core_M3Vec4push_Gi32` (monomorphized for Vec<i32>)

### Closures

```rask
func main() {
    const f = |x: i32| -> i32 { x + 1 }
    const g = |y: i32| -> i32 { y * 2 }
}
```
→ `_R4main_L0_H3a2f` (first closure)
→ `_R4main_L1_H7b4e` (second closure)

### Tests and Benchmarks

```rask
test "parse URL correctly" {
    // ...
}

benchmark "JSON decode" {
    // ...
}
```
→ `_R5myapp_Test17parse_URL_correctly`
→ `_R5myapp_Bench11JSON_decode`

Test/benchmark names: replace spaces with underscores, keep alphanumeric+underscore, drop other chars.

### Context Clauses

Context clauses included in generic args:

```rask
func write(h: Handle<T>) using Pool<T>
```
→ `_R4core_F5write_GHandle[T]Pool[T]`

### Main Entry Point

```rask
func main()
```
→ `main` (no mangling, standard entry point)

Special functions with `@entry` attribute use unmangled symbol:
```rask
@entry("start")
func custom_entry()
```
→ `start`

## Demangling

Demangling algorithm:
1. Check `_R` prefix
2. Read length-prefixed segments
3. Parse type markers
4. Decode generic args with bracket nesting
5. Format for display

```
_R4core_F4sort_GVec[i32]Compare[i32]_H3a2f
↓
core::sort<Vec<i32>, Compare<i32>>#3a2f
```

## FFI and Extern

```rask
extern "C" func malloc(size: usize) -> *u8
```
→ `malloc` (no mangling, respect ABI)

Rask functions exposed to C via `@export("name")`:
```rask
@export("rask_init")
public func initialize()
```
→ `rask_init` (no mangling, user-specified symbol)

## Runtime Functions

Built-in runtime functions use reserved prefix `_Rrt`:

| Function | Symbol |
|----------|--------|
| Allocator | `_Rrt_alloc`, `_Rrt_dealloc` |
| Panic | `_Rrt_panic` |
| Vec ops | `_Rrt_vec_push`, `_Rrt_vec_grow` |
| Pool ops | `_Rrt_pool_alloc`, `_Rrt_pool_free` |
| Spawn | `_Rrt_spawn`, `_Rrt_spawn_detach` |

## Implementation Notes

The mangling scheme prioritizes:
- **Readability**: Length-prefixed segments better than base64 encoding
- **Debuggability**: Type info preserved in symbol for better stack traces
- **Simplicity**: No compression schemes or complex encoding rules
- **Tooling**: Easy to write demangler, grep for symbols

Collision hashes kept short (4 chars) since they're rare — most symbols won't need them.

Type encoding uses brackets for generics rather than angle brackets to avoid shell escaping issues when grepping object files.

## Error Messages

When symbol collision detected during codegen:

```
ERROR [compiler.mangling/H1]: Symbol collision detected
  |
5 | func process<T>(x: T)
  |      ^^^^^^^ conflicts with existing definition

WHY: Two different monomorphizations produced the same symbol name.
     This is a compiler bug — collision hashes should prevent this.

FIX: Report this as a compiler bug with the conflicting types.
```

## See Also

- `type.generics` — Generic type parameters and constraints
- `struct.modules` — Package organization and visibility
- `compiler.mir` — MIR representation (not yet written)
