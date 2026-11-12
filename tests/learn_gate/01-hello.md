# Hello

The function this chapter is about:

<!-- from: mud/greet.rk -->
```rask
public func greet(name: string) -> string {
    return "Hello, {name}!"
}
```

The exercise fails until you fill it in:

```bash
rask test 01-hello.rk
```

<!-- from: output/01-test.out -->
```text
=== Testing 01-hello.rk ===

  ✗ greets by name
      01-hello.rk:11: not yet implemented

──────────────────────────────────────────────────
1 tests, 0 passed, 1 failed (0ms)
```

The finished program passes:

```bash
rask test mud
```

<!-- from: output/01-test-solved.out -->
```text
=== Testing mud ===

  ✓ greets by name (0ms)

──────────────────────────────────────────────────
1 tests, 1 passed, 0 failed (0ms)
```

Installing isn't run by the gate:

<!-- learn: skip -->
```bash
curl -fsSL https://rask-lang.dev/install.sh | sh
```
