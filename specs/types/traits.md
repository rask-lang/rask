# Solution: Runtime Polymorphism

## The Question
How do you store different types in the same collection when they share behavior? How does Rask support heterogeneous data structures?

## Decision
Opt-in runtime polymorphism via `any Trait`. Monomorphization remains the default; `any` is used when you explicitly need different types in the same collection.

## Rationale
Most code benefits from monomorphization (zero overhead, full optimization). But some patterns—HTTP handlers, UI widgets, plugin systems—fundamentally need heterogeneous collections. `any Trait` provides this capability with explicit, visible runtime cost.

## Specification

### The Problem

With monomorphization only, all items in a collection must be the same type:

```rask
trait Widget {
    draw(self)
}

// This works: all Buttons
let buttons: []Button = [button1, button2, button3]
for b in buttons { b.draw() }

// This FAILS: different types
const widgets = [button, textbox, slider]  // ERROR: types don't match
```

### The Solution: `any Trait`

`any Trait` creates a type-erased value that can hold any type satisfying the trait:

```rask
// Different types in the same collection
let widgets: []any Widget = [button, textbox, slider]
for w in widgets {
    w.draw()  // Dispatches to correct implementation at runtime
}
```

### How It Works

A `any Trait` value consists of two parts:
1. **Data**: The actual value (or pointer to it)
2. **Vtable**: A table of function pointers for the trait's methods

```rask
┌─────────────┐
│ any Widget  │
├─────────────┤
│ data ───────┼──► actual Button/TextBox/Slider value
│ vtable ─────┼──► [draw_ptr, ...]
└─────────────┘
```

When you call `w.draw()`, the runtime:
1. Looks up the `draw` function pointer in the vtable
2. Calls it with the data

This is called **dynamic dispatch** or **vtable dispatch**.

### When to Use `any Trait`

| Use Case | Example | Why `any` |
|----------|---------|-----------|
| **HTTP handlers** | `[]any Handler` | Different handlers for different routes |
| **UI widgets** | `[]any Widget` | Mix buttons, text, sliders in one view |
| **Plugin systems** | `[]any Plugin` | Load unknown types at runtime |
| **Event listeners** | `[]any Listener` | Different callbacks for same event |
| **Heterogeneous caches** | `Map<Key, any Value>` | Store different value types |

### When NOT to Use `any Trait`

| Situation | Use Instead |
|-----------|-------------|
| All items same type | Regular generics `[]T` |
| Known set of types | Enum with variants |
| Performance critical hot loop | Monomorphization or enum |
| Need to access type-specific fields | Enum or separate collections |

### Syntax

**Creating `any` values:**
```rask
let w: any Widget = button        // Implicit conversion
const w = button as any Widget      // Explicit conversion
```

**Collections of `any`:**
```rask
let widgets: []any Widget = [button, textbox]
let handlers: Map<string, any Handler> = ...
```

**Function parameters:**
<!-- test: parse -->
```rask
func render(widget: any Widget) {
    widget.draw()
}

func render_all(widgets: []any Widget) {
    for w in widgets { w.draw() }
}
```

### Trait Requirements for `any`

Not all traits can be used with `any`. The trait must be **object-safe**:

| Allowed | Not Allowed |
|---------|-------------|
| Methods with `self` parameter | Methods returning `Self` |
| Methods with concrete types | Generic methods |
| Methods with trait constraints | Associated types (MVP) |

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

### Cost

| Aspect | Monomorphization | `any Trait` |
|--------|------------------|-------------|
| Method call | Direct call | Indirect (vtable lookup) |
| Inlining | Yes | No |
| Code size | One copy per type | One copy total |
| Flexibility | Same-type only | Heterogeneous |

The overhead is small—one pointer indirection per method call. For most applications (UI, handlers, plugins) this is negligible. For tight inner loops, prefer monomorphization or enums.

### Comparison with Enums

Both `any Trait` and enums can hold different types. Choose based on:

| | `any Trait` | Enum |
|---|-------------|------|
| Open/extensible | ✅ Add new types anytime | ❌ Fixed set of variants |
| Pattern matching | ❌ Cannot match on type | ✅ Full pattern matching |
| Access fields | ❌ Only trait methods | ✅ Direct field access |
| External types | ✅ Works with any type | ❌ Must be defined in enum |

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

### Plugin System

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

## Integration Notes

- **Memory model**: `any` values own their data; same ownership rules as regular values
- **Clone**: `any Trait` is NOT automatically Clone (would require trait method)
- **Sized**: `any Trait` is unsized; usually stored behind a pointer or in a collection
- **Concurrency**: `any` values can be sent between tasks if the underlying type is sendable
