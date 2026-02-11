<!-- id: type.traits -->
<!-- status: decided -->
<!-- summary: Opt-in runtime polymorphism via `any Trait` with vtable dispatch -->
<!-- depends: types/structs.md, types/generics.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Traits

Monomorphization by default; `any Trait` for explicit runtime polymorphism when different types share a collection.

## Object Safety

| Rule | Description |
|------|-------------|
| **TR1: Object-safe methods** | Methods must take `self` and use only concrete types |
| **TR2: No Self return** | Methods returning `Self` prevent `any` usage |
| **TR3: No generic methods** | Generic methods prevent `any` usage |
| **TR4: No associated types** | Associated types prevent `any` usage (MVP) |

<!-- test: parse -->
```rask
// Object-safe: can use with any
trait Widget {
    draw(self)
    size(self) -> (i32, i32)
}

// NOT object-safe: cannot use with any
trait Clonable {
    clone(self) -> Self  // Returns Self
}

trait Container {
    get<T>(self, key: T) -> T  // Generic method
}
```

## Syntax

| Rule | Description |
|------|-------------|
| **TR5: Implicit conversion** | `let w: any Widget = button` — converts at assignment |
| **TR6: Explicit cast** | `const w = button as any Widget` — converts with `as` |
| **TR7: Collection type** | `[]any Widget`, `Map<string, any Handler>` — heterogeneous collections |
| **TR8: Parameter type** | `func f(w: any Widget)` — accepts any implementor |

<!-- test: parse -->
```rask
func render(widget: any Widget) {
    widget.draw()
}

func render_all(widgets: []any Widget) {
    for w in widgets { w.draw() }
}
```

## Dispatch

| Rule | Description |
|------|-------------|
| **TR9: Vtable dispatch** | `any Trait` method calls go through a vtable (function pointer table) |
| **TR10: Two-word value** | `any Trait` stores a data pointer and a vtable pointer |

## Cost

| Aspect | Monomorphization | `any Trait` |
|--------|------------------|-------------|
| Method call | Direct call | Indirect (vtable lookup) |
| Inlining | Yes | No |
| Code size | One copy per type | One copy total |
| Flexibility | Same-type only | Heterogeneous |

Overhead is one pointer indirection per call. Negligible for handlers, UI, plugins. For tight inner loops, prefer monomorphization or enums.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Trait with `Self` return | TR2 | Compiler rejects `any` usage |
| Trait with generic method | TR3 | Compiler rejects `any` usage |
| Clone of `any` value | — | Not automatic; requires explicit trait method |
| Concurrency | — | `any` values sendable if underlying type is sendable |
| Sizing | — | `any Trait` is unsized; stored behind pointer or in collection |

## Error Messages

**Using non-object-safe trait with `any` [TR2]:**
```
ERROR [type.traits/TR2]: trait `Clonable` is not object-safe
   |
5  |  let c: any Clonable = value
   |             ^^^^^^^^ `clone` returns `Self`

WHY: `any` erases the concrete type, so methods returning `Self`
     can't know what type to return.

FIX: Use an enum or generic constraint instead:

  func do_clone<T: Clonable>(value: T) -> T {
      return value.clone()
  }
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

**TR1–TR4 (object safety):** `any` erases the concrete type at runtime. Methods that depend on knowing the concrete type — returning `Self`, using generic parameters — can't work through a vtable. These restrictions match what's mechanically possible, not arbitrary limits.

**TR9 (vtable dispatch):** The cost is explicit. You write `any Trait`, you get indirection. No hidden polymorphism, no surprise performance cliffs. Monomorphization remains the default for zero-overhead generics.

### Patterns & Guidance

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
| Performance critical hot loop | Monomorphization or enum |
| Need type-specific fields | Enum or separate collections |

**Comparison with enums:**

| | `any Trait` | Enum |
|---|-------------|------|
| Open/extensible | Yes — add new types anytime | No — fixed set of variants |
| Pattern matching | No — only trait methods | Yes — full pattern matching |
| Access fields | No — only trait methods | Yes — direct field access |
| External types | Yes — works with any type | No — must be defined in enum |

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
1. **Data**: Actual value (or pointer to it)
2. **Vtable**: Function pointers for trait methods

```
┌─────────────┐
│ any Widget  │
├─────────────┤
│ data ───────┼──► actual Button/TextBox/Slider value
│ vtable ─────┼──► [draw_ptr, ...]
└─────────────┘
```

When you call `w.draw()`, the runtime looks up the `draw` function pointer in the vtable and calls it with the data.

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

- **Ownership**: `any` values own their data; same ownership rules as regular values (`mem.ownership`)
- **Clone**: `any Trait` is NOT automatically cloneable — requires an explicit trait method
- **Sized**: `any Trait` is unsized; usually stored behind a pointer or in a collection
- **Concurrency**: `any` values can be sent between tasks if the underlying type is sendable (`conc.tasks`)

### See Also

- [Structs](structs.md) — Method syntax, `extend` blocks (`type.structs`)
- [Generics](generics.md) — Monomorphization, trait bounds (`type.generics`)
- [Enums](enums.md) — Closed-set alternative (`type.enums`)
- [Ownership](../memory/ownership.md) — Value ownership model (`mem.ownership`)
