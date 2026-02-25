# Glossary

Quick reference for technical terms used across specs. Each term is explained where it first appears — this page is for lookup.

| Term | Plain meaning | Spec |
|------|---------------|------|
| **Block-scoped** (view/borrow) | A temporary reference valid until the end of the enclosing `{ }` block. Used for fixed-layout data (struct fields, arrays). Strings are NOT block-scoped — they own heap buffers and use inline access. | `mem.borrowing` |
| **Borrow** / **View** | A temporary reference to data you don't own. The compiler tracks it so it can't outlive the data or conflict with mutations. | `mem.borrowing` |
| **Capture** (closures) | When a closure uses a variable from outside its body, that variable is *captured* — copied or moved into the closure so it's available when the closure runs later. | `mem.closures` |
| **Desugaring** | Expanding syntactic shortcuts into their full form before the compiler processes them. Example: `a + b` becomes `a.add(b)`. | `comp.semantic-hash` |
| **Exclusive access** | Only one mutable reference exists, and no other references (read or write) can coexist. Prevents data races. Sometimes called "aliasing XOR mutation." | `mem.borrowing` |
| **Monomorphization** | When you call a generic function like `sort<i32>`, the compiler generates a specialized version of `sort` just for `i32`. Fast (direct calls, no runtime type checks) but increases binary size. | `type.generics` |
| **Move** | Transferring a value from one variable to another. After a move, the original variable is unusable — the compiler forbids reading it. Only applies to types larger than 16 bytes. | `mem.ownership` |
| **Must-consume** (resource types) | A type marked `@resource` that the compiler forces you to use exactly once — you can't forget to close a file or commit a transaction. Sometimes called "linear types" in academic literature. | `mem.resources` |
| **Must-use** (task handles) | `TaskHandle` must be explicitly joined (wait for result) or detached (fire-and-forget). Dropping it without doing either is a compile error. Sometimes called "affine" in type theory. | `conc.async` |
| **Inline access** | Temporary access to a collection element that lasts only for the expression. Used for growable sources (Vec, Pool, Map, string) because they might reallocate. Multi-statement access uses `with...as`. | `mem.borrowing` |
| **`with` block** | First-class block scope for multi-statement access to collection elements, Cell, Shared, and Mutex values. `return`, `try`, `break`, `continue` work naturally. | `mem.borrowing` |
| **Structural trait matching** | The compiler checks if a type has all the methods a trait requires — matching by shape, not by explicit declaration. If your type has a `compare(self, other: T) -> Ordering` method, it satisfies `Comparable` automatically. | `type.generics` |
| **Vtable** | A table of function pointers, one per trait method. When you use `any Trait`, the runtime looks up the right function in this table and calls it. Costs one pointer indirection per call. | `type.traits` |
