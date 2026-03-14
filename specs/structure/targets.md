<!-- id: struct.targets -->
<!-- status: decided -->
<!-- summary: Package role from func main() presence; @entry for non-main entry points -->
<!-- depends: structure/modules.md, structure/build.md -->

# Libraries vs Executables

Package role is determined by presence of `func main()`. No manifest flags, no dual-purpose packages. A package is either a library or an executable — never both. `@entry` is optional — only needed for non-main entry points.

## Package Classification

| Rule | Description |
|------|-------------|
| **PC1: Executable** | Package with `func main()` or `@entry` → binary |
| **PC2: Library** | Package without entry point → imported only |
| **PC3: One entry** | Exactly one entry point per program — multiple is a compile error |
| **PC4: Root only** | Entry point must be in root package directory, not nested packages |

| Pattern | Classification | Output |
|---------|---------------|--------|
| Package with `func main()` or `@entry` | Executable | Binary |
| Package without entry point | Library | None |
| Package with `*_test.rk` | Library + tests | Test binary |

## Entry Point Signatures

| Rule | Description |
|------|-------------|
| **EP1: Public required** | Entry function must be `public` |
| **EP2: Conventional name** | `func main()` is convention — no annotation needed |
| **EP3: @entry** | `@entry` marks a non-main function as entry point |

| Signature | When to Use |
|-----------|-------------|
| `public func main()` | Sync program, infallible |
| `public func main() -> () or Error` | Sync program, can fail |
| `public func main(args: Args)` | Needs CLI arguments |

## CLI Arguments

| Rule | Description |
|------|-------------|
| **AR1: Args type** | `Args` is a built-in type, always available |
| **AR2: Program name** | `args[0]` is program name (like C) |
| **AR3: UTF-8** | Arguments are always valid UTF-8 |

<!-- test: parse -->
```rask
public func main(args: Args) {
    for arg in args {
        print(arg)
    }
}
```

## Standard Streams

| Rule | Description |
|------|-------------|
| **SS1: Built-in handles** | `stdin`, `stdout`, `stderr` available in `main()` scope |
| **SS2: Linear resources** | Must be consumed exactly once |
| **SS3: Not global** | Not available in other functions — pass as parameters |

## Process Exit

| Rule | Description |
|------|-------------|
| **EX1: Normal return** | `main` returning → status 0 |
| **EX2: Error return** | `main` returning `Err(e)` → status 1, error to stderr |
| **EX3: Explicit exit** | `sys.exit(n)` → immediate, no cleanup |
| **EX4: Panic** | Panic → status 101, message to stderr |
| **EX5: Ensure runs** | `ensure` blocks run before exit (unless `sys.exit()`) |

## One Package, One Role

| Rule | Description |
|------|-------------|
| **MB1: No dual-purpose** | A package with `func main()` is an executable. A package without is a library. Never both |
| **MB2: Lib + CLI pattern** | Library package + thin executable package in a workspace. The executable imports the library |
| **MB3: Multi-binary pattern** | Multiple executable packages in a workspace, each importing shared library packages |

```
mylib/
  build.rk            # members: ["mylib", "cli"]
  mylib/
    parser.rk          # public API — library
    format.rk
  cli/
    main.rk            # import mylib — executable wrapper
```

Multiple binaries:

```
myproject/
  build.rk            # members: ["core", "client", "server"]
  core/
    protocol.rk        # shared library
  client/
    main.rk            # import core
  server/
    main.rk            # import core
```

## Error Messages

```
ERROR [struct.targets/PC3]: multiple entry points
   |
5  |  public func main() { }
   |  ^^^^^^^^^^^^^^^^^ entry point already defined at server.rk:3
```

```
ERROR [struct.targets/EP1]: entry point not public
   |
3  |  func main() { }
   |  ^^^^^^^^^^^ entry point must be `public`
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Entry not `public` | EP1 | Compile error |
| Multiple entry points | PC3 | Compile error |
| Entry in nested package | PC4 | Compile error |
| Both `main()` and `@entry` | PC3 | Compile error: ambiguous |
| `sys.exit()` with unconsumed linear resource | EX3 | Resource leaked |
| `init()` failure before `main()` | — | Entry function never runs |
| Importing an executable package | MB1 | Compile error — executables are not importable |

---

## Appendix (non-normative)

### Rationale

**PC1 (main() convention):** `func main()` is universal (C, Go, Rust, Java). No build file needed for basic usage. Follows "package = directory" — structure determines behavior.

**EP3 (@entry):** Exists for the rare case where `main` conflicts with a domain term. Not needed in practice.

**MB1 (no dual-purpose):** Rust allows `src/lib.rs` + `src/main.rs` in one crate — a package that's both library and executable. This creates ambiguity: what does "import X" mean when X has a `main()`? I chose one package, one role. The cost is a thin wrapper directory for the CLI case. The benefit is that "library" and "executable" are always unambiguous from the package structure. Simple over easy.

### Examples

**Minimal executable:**
<!-- test: parse -->
```rask
public func main() {
    print("hello")
}
```

**Library:**
<!-- test: skip -->
```rask
public struct Request { ... }
public func new(method: string, path: string) -> Request { ... }
// No func main() → library
```

**Library + CLI (workspace):**
```
mylib/
  build.rk          // members: ["mylib", "cli"]
  mylib/
    core.rk
  cli/
    main.rk         // import mylib; thin wrapper
  examples/
    basic.rk        // func main()
    advanced.rk     // func main()
```

### Comparison

| Language | Distinction | Entry Point |
|----------|-------------|-------------|
| Rask | Presence of `func main()` | `public func main()` |
| Rust | `Cargo.toml` sections | `fn main()` |
| Go | Package name | `func main()` in package main |
| Zig | Build script | `pub fn main()` |

### See Also

- `struct.modules` — visibility, package organization
- `struct.build` — build configuration, workspaces
