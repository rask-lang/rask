---
paths:
  - "stdlib/**"
  - "compiler/crates/rask-stdlib/**"
  - "specs/stdlib/**"
---

# Stdlib design

The stdlib should feel Rask, not Rust-with-different-keywords. Don't lift names,
shapes, or layering from `std::*` just because they're familiar.

- Pick names from how Rask programs read, not from Rust precedent. `Vec`/`Map`
  survive because they fit; `Result`, `Option`, `Box`, `Rc`, `RefCell`,
  `Arc<Mutex<T>>` do not. Rask has `T or E`, `T?`, `Heap`, `Shared<T, S>`.
- Method names follow Rask conventions, not Rust's (`unwrap`, `expect`, `ok_or`,
  `and_then` are Rust idioms; design from the actual operation, not the cheat sheet).
- Layering should reflect `Shared`/`Rack`/`Heap` and the linearity rules. Don't
  import Rust's trait hierarchy (`Deref`, `Borrow`, `AsRef`, `Iterator` adapters)
  by reflex.
- When in doubt, sketch how the call site reads in a real Rask program first,
  then pick the name.

If a Rust name genuinely is the right one, fine, but justify it from Rask's side,
not from "that's what `std` calls it."

Formal rules (one screen per module, the guess test, SD1–SD5):
[specs/stdlib/api-design.md](../../specs/stdlib/api-design.md).
