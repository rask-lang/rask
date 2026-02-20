<!-- id: type.traits -->
<!-- status: decided -->
<!-- summary: Opt-in runtime polymorphism via `any Trait` with function pointer dispatch -->
<!-- depends: types/structs.md, types/generics.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Traits

Code specialization by default — each type gets its own optimized copy. Use `any Trait` for explicit runtime polymorphism when different types need to share a collection.

## Which Methods Work Through `any`

Any trait can be used with `any`. Individual methods that depend on the concrete type can't be called through `any` — the compiler rejects them at the call site, not when creating the `any` value.

| Rule | Description |
|------|-------------|
| **TR1: Per-method restriction** | Methods are checked individually; incompatible methods can't be called through `any`, but don't prevent using the trait with `any` |
| **TR2: No Self return** | Methods returning `Self` can't be called through `any` |
| **TR3: No generic methods** | Generic methods can't be called through `any` |
| **TR4: No associated types** | Methods using associated types can't be called through `any` (MVP) |

<!-- test: parse -->
```rask
trait Clonable {
    clone(self) -> Self       // can't call through any (returns Self)
    name(self) -> string      // fine — concrete return type
}

const c: any Clonable = foo
c.name()                      // OK
```

The vtable only contains slots for compatible methods. Incompatible methods have no vtable entry.

## Syntax

| Rule | Description |
|------|-------------|
| **TR5: Implicit at assignment** | `let w: any Widget = button` — the `any` type annotation signals the conversion |
| **TR6: Explicit cast** | `const w = button as any Widget` — converts with `as` |
| **TR7: Collection type** | `[]any Widget`, `Map<string, any Handler>` — heterogeneous collections |
| **TR8: Explicit at call sites** | Passing a concrete value to a function taking `any Trait` requires `as any Trait` |

<!-- test: parse -->
```rask
func render(widget: any Widget) {
    widget.draw()
}

func render_all(widgets: []any Widget) {
    for w in widgets { w.draw() }
}
```

TR8 means concrete values need explicit conversion at call sites:

<!-- test: skip -->
```rask
const button = Button { label: "OK" }

render(button as any Widget)   // OK: explicit conversion
render(button)                 // ERROR: implicit coercion at call site

const w: any Widget = button   // OK: type annotation signals it (TR5)
render(w)                      // OK: w is already any Widget
```

## Boxing

Creating an `any Trait` value heap-allocates the concrete data.

| Rule | Description |
|------|-------------|
| **TR9: Heap allocation** | `any Trait` heap-allocates the concrete value and constructs a fat pointer (data pointer + vtable pointer) |
| **TR10: Owned data** | `any Trait` owns its heap data — same ownership model as Vec or string |
| **TR11: Move-only** | `any Trait` is never Copy; assignment moves. Clone only if the trait provides a clone method |

The `any` keyword in the type is the cost signal.

## Dispatch

| Rule | Description |
|------|-------------|
| **TR12: Vtable dispatch** | `any Trait` method calls go through a vtable — a table of function pointers, one per compatible method |
| **TR13: Two-word value** | `any Trait` is a fat pointer: data pointer and vtable pointer (16 bytes) |

## Drop

| Rule | Description |
|------|-------------|
| **TR14: Scope cleanup** | When `any Trait` goes out of scope: call the vtable's `drop_fn(data_ptr)` if non-null, then free the heap allocation |
| **TR15: discard** | `discard` on `any Trait` triggers the same cleanup as scope exit |
| **TR16: Collection cleanup** | Dropping a collection of `any Trait` values drops each element individually through its vtable before freeing the collection |

## Cost

| Aspect | Specialized code | `any Trait` |
|--------|------------------|-------------|
| Method call | Direct call | Indirect (vtable lookup) |
| Inlining | Yes | No |
| Code size | One copy per type | One copy total |
| Memory | Value inline | Heap-allocated + pointer |
| Flexibility | Same-type only | Heterogeneous |

Overhead is one pointer indirection per call plus a heap allocation per value. Negligible for handlers, UI, plugins. For tight inner loops, prefer generics (specialized code) or enums.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Method returns `Self` | TR2 | Can't call through `any`; other methods still work |
| Generic method | TR3 | Can't call through `any`; other methods still work |
| Clone of `any` value | TR11 | Not automatic; requires explicit trait method |
| Assignment | TR11 | Moves (never copies) |
| Concurrency | — | `any` values sendable if underlying type is sendable |
| Pool element | — | Not supported; use `[]any Trait` for heterogeneous collections |

## Error Messages

**Calling incompatible method through `any` [TR2]:**
```
ERROR [type.traits/TR2]: `clone` can't be called through `any`
   |
8  |  c.clone()
   |    ^^^^^ returns `Self`, which is erased by `any`

WHY: `any` erases the concrete type. Methods returning `Self`
     can't know what type to return.

FIX: Use a generic function for type-preserving operations:

  func do_clone<T: Clonable>(value: T) -> T {
      return value.clone()
  }
```

**Implicit coercion at call site [TR8]:**
```
ERROR [type.traits/TR8]: can't implicitly convert `Button` to `any Widget`
   |
5  |  render(button)
   |         ^^^^^^ expected `any Widget`, found `Button`

FIX: Use explicit conversion:

  render(button as any Widget)
```

## Examples

### HTTP Router

<!-- test: parse -->
```rask
trait Handler {
    handle(self, req: Request) -> Response
}

struct Router {
    routes: Map<string, any Handler>
}

extend Router {
    func add(self, path: string, handler: any Handler) {
        self.routes.insert(path, handler)
    }

    func dispatch(self, req: Request) -> Response {
        match self.routes.get(req.path) {
            Some(handler) => handler.handle(req),
            None => Response.not_found(),
        }
    }
}
```

### UI Widget Tree

<!-- test: parse -->
```rask
trait Widget {
    draw(self, canvas: Canvas)
    size(self) -> (i32, i32)
}

struct Container {
    children: []any Widget
}

extend Container {
    func draw(self, canvas: Canvas) {
        for child in self.children {
            child.draw(canvas)
        }
    }
}
```

---

## Appendix (non-normative)

### Rationale

**TR1–TR4 (per-method restrictions):** Rust rejects entire traits from `dyn` if any method is incompatible — "trait is not object-safe." I think that's too coarse. A trait with nine compatible methods and one `Self`-returning method should work with `any` — you just can't call that one method. The error appears at the call site where the problem is, not at the coercion site where it isn't.

**TR8 (explicit at call sites):** `render(button)` where `render` takes `any Widget` hides a heap allocation behind what looks like a regular function call. I'd rather make you write `render(button as any Widget)` — the `any` keyword should always appear in the code wherever a conversion happens. Assignment conversion (TR5) is implicit because the `any Widget` type annotation is already visible.

**TR9 (heap allocation):** I chose owned heap allocation over alternatives. The `any` keyword is the cost signal — you see it in the type, you know there's indirection and allocation. This is a deliberate tradeoff: ergonomic for the use cases where you need it (handlers, plugins, UI), explicit enough that you won't accidentally use it in hot paths.

**TR12 (vtable dispatch):** The cost is explicit. You write `any Trait`, you get indirection. No hidden polymorphism, no surprise performance cliffs. Specialized code generation remains the default for zero-overhead generics.

### Patterns & Guidance

**Prefer enums and closures before reaching for `any Trait`:**

| Need | Use | Why |
|------|-----|-----|
| Known set of types | Enum | Zero overhead, pattern matching, field access |
| Single shared method | `Func(Args) -> Ret` | No vtable, just a function pointer |
| Open set, multi-method | `any Trait` | When enums and closures don't fit |

**When to use `any Trait`:**

| Use Case | Example | Why `any` |
|----------|---------|-----------|
| HTTP handlers | `[]any Handler` | Different handlers for different routes |
| UI widgets | `[]any Widget` | Mix buttons, text, sliders in one view |
| Plugin systems | `[]any Plugin` | Load unknown types at runtime |
| Event listeners | `[]any Listener` | Different callbacks for same event |
| Heterogeneous caches | `Map<Key, any Value>` | Store different value types |

**When NOT to use `any Trait`:**

| Situation | Use Instead |
|-----------|-------------|
| All items same type | Regular generics `[]T` |
| Known set of types | Enum with variants |
| Performance critical hot loop | Generics (specialized code) or enum |
| Need type-specific fields | Enum or separate collections |
| Single method interface | Function value: `Func(Request) -> Response` |

**Comparison with enums:**

| | `any Trait` | Enum |
|---|-------------|------|
| Open/extensible | Yes — add new types anytime | No — fixed set of variants |
| Pattern matching | No — only trait methods | Yes — full pattern matching |
| Access fields | No — only trait methods | Yes — direct field access |
| External types | Yes — works with any type | No — must be defined in enum |
| Memory | Heap allocation per value | Inline (tag + union) |

```rask
// Enum: closed set, full access
enum Shape {
    Circle { radius: f32 }
    Rect { w: f32, h: f32 }
}
match shape {
    Circle { radius } => ...  // Access fields
    Rect { w, h } => ...
}

// any: open set, methods only
let shapes: []any Drawable = [circle, rect, custom_shape]
for s in shapes { s.draw() }  // Only trait methods
```

**How vtable dispatch works:**

An `any Trait` value has two parts:
1. **Data**: Pointer to heap-allocated concrete value
2. **Vtable**: Pointer to static function pointer table

```
┌─────────────┐
│ any Widget  │
├─────────────┤
│ data ───────┼──► [heap: concrete Button/TextBox/Slider value]
│ vtable ─────┼──► [draw_ptr, size_ptr, ...]
└─────────────┘
```

When you call `w.draw()`, the runtime loads the `draw` function pointer from the vtable and calls it with the data pointer.

### Collection Thinning (implementation note)

Collections of `any Trait` values can use a thin pointer optimization. Owned `any Trait` values heap-allocate with the vtable pointer as a header:

```
Heap block:  [vtable_ptr | concrete_data...]
              ^            ^
              base         base + 8
```

The fat pointer is `(data_ptr = base + 8, vtable_ptr)`. Collections can store just the base address (8 bytes instead of 16). Reading an element inflates it back to a fat pointer: `data = base + 8, vtable = *(base)`.

Borrowed fat pointers (function parameters where data_ptr points to stack data) don't have the vtable header — but borrowed values can't be stored in collections, so this is safe. Rask's ownership rules enforce the invariant. Half the element size, same spec-level model.

### Plugin System Example

<!-- test: parse -->
```rask
trait Plugin {
    name(self) -> string
    init(self)
    run(self, ctx: Context)
}

struct App {
    plugins: []any Plugin
}

extend App {
    func load_plugin(self, plugin: any Plugin) {
        plugin.init()
        self.plugins.push(plugin)
    }
}
```

### Integration Notes

- **Ownership**: `any` values own their heap data — the data pointer is owned, not a reference (`mem.ownership`)
- **Clone**: `any Trait` is NOT automatically cloneable — requires an explicit trait method
- **Drop**: scope exit calls vtable `drop_fn` then frees the heap allocation (`TR14`)
- **Concurrency**: `any` values can be sent between tasks if the underlying type is sendable (`conc.tasks`)

### See Also

- [Structs](structs.md) — Method syntax, `extend` blocks (`type.structs`)
- [Generics](generics.md) — Code specialization, trait bounds (`type.generics`)
- [Enums](enums.md) — Closed-set alternative (`type.enums`)
- [Ownership](../memory/ownership.md) — Value ownership model (`mem.ownership`)
- [Value Semantics](../memory/value-semantics.md) — Copy vs move, 16-byte threshold (`mem.value`)
