# Interpreter Deep Dive

How `rask run` executes programs without compiling them. This is a **tree-walk
interpreter**: it directly walks the AST, evaluating nodes as it goes.

The interpreter lives in `rask-interp/`. After all the analysis stages
(lex → parse → desugar → resolve → typecheck → ownership), the validated AST
is handed to the interpreter for execution.


## Architecture

```
rask-interp/src/
├── lib.rs              Re-exports
├── value.rs            Runtime value representation
├── env.rs              Variable scope management
├── resource.rs         Linear resource tracker
└── interp/
    ├── mod.rs          Interpreter struct, run(), run_tests()
    ├── register.rs     Register declarations into lookup tables
    ├── eval_expr.rs    Evaluate expressions → Value
    ├── exec_stmt.rs    Execute statements for side effects
    ├── call.rs         Function call mechanics
    ├── dispatch.rs     Method dispatch (builtin → user-defined)
    ├── assign.rs       Assignment (field paths, indexing)
    ├── pattern.rs      Pattern matching logic
    ├── collections.rs  Vec/Map built-in operations
    ├── operators.rs    Desugared operator method dispatch
    ├── format.rs       String interpolation
    └── monomorphize.rs Runtime generic instantiation
```

Plus built-in methods and stdlib modules:

```
├── builtins/
│   ├── mod.rs          Dispatch to specific type builtins
│   ├── primitives.rs   i32.add, f64.sqrt, bool.not, etc.
│   ├── strings.rs      string.len, .contains, .split, etc.
│   ├── collections.rs  Vec.push, .pop, Map.insert, etc.
│   ├── enums.rs        Option/Result methods
│   ├── shared.rs       Shared<T> (atomic refcount) methods
│   └── threading.rs    Thread, ThreadPool, Mutex, Atomic methods
└── stdlib/
    ├── mod.rs
    ├── fs.rs           File I/O
    ├── net.rs          TCP sockets
    ├── io.rs           stdin/stdout
    ├── json.rs         JSON parse/stringify
    ├── time.rs         Instant, Duration, sleep
    ├── random.rs       RNG
    ├── math.rs         sin, cos, PI, etc.
    ├── os.rs           Environment, platform, exit
    ├── cli.rs          Argument parsing
    ├── env.rs          Environment variables
    ├── path.rs         Path manipulation
    ├── async_mod.rs    Green task spawning
    └── thread.rs       OS thread spawning
```


## The Interpreter struct

```rust
pub struct Interpreter {
    env: Environment,
    functions: HashMap<String, FnDecl>,
    enums: HashMap<String, EnumDecl>,
    struct_decls: HashMap<String, StructDecl>,
    monomorphized_structs: HashMap<String, StructDecl>,
    methods: HashMap<String, HashMap<String, FnDecl>>,
    resource_tracker: ResourceTracker,
    output_buffer: Option<Arc<Mutex<String>>>,
    cli_args: Vec<String>,
}
```

**`functions`** maps name → AST declaration. When you call `foo()`, the
interpreter looks up `foo` here, gets its `FnDecl`, and executes its body.

**`methods`** is a two-level map: type name → method name → `FnDecl`. Populated
from `extend` blocks. When you call `point.distance(other)`, it looks up
`"Point"` → `"distance"`.

**`monomorphized_structs`** handles runtime generic instantiation. When you
write `Buffer<i32, 256>`, the interpreter creates a concrete struct declaration
on the fly and caches it here.

**`output_buffer`** is for testing. When running `rask test`, output is
captured into this buffer for assertion checking instead of going to stdout.


## Values

`value.rs` defines what exists at runtime:

```rust
enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    String(Arc<Mutex<String>>),
    Unit,

    // Compound
    Struct { name: String, fields: HashMap<String, Value> },
    Enum { enum_name: String, variant: String, data: Vec<Value> },
    Vec(Arc<Mutex<Vec<Value>>>),
    Map(Arc<Mutex<IndexMap<String, Value>>>),
    Tuple(Vec<Value>),

    // Functions
    Function { name: String },
    Closure { params, body, captured: HashMap<String, Value> },
    Builtin(BuiltinKind),

    // Types as values (for static methods: Vec.new(), string.new())
    TypeConstructor { kind: TypeConstructorKind, type_param: Option<String> },
    Module(ModuleKind),

    // Concurrency
    Pool(Arc<Mutex<PoolData>>),
    Handle { pool_id, index, generation },
    Channel { sender, receiver },
    Shared(Arc<RwLock<Value>>),
    ThreadHandle(Arc<ThreadHandleInner>),
    // ...
}
```

**Why `Arc<Mutex<String>>` for strings?** Strings are reference-counted and
shareable across threads. The `Arc` is the reference count, `Mutex` is the
lock. Same pattern for `Vec` and `Map`.

**Closures capture by value.** When a closure is created, `env.capture()`
copies all visible variables into the closure's `captured` map. This is
simpler than tracking references but means closures can't mutate outer
variables (which matches Rask's ownership model).


## The Environment

`env.rs` manages variable scopes:

```rust
pub struct Environment {
    scopes: Vec<Scope>,  // stack of scopes
}

struct Scope {
    bindings: HashMap<String, Value>,
}
```

- **`push_scope()`**: Enter a new scope (function body, block, loop body)
- **`pop_scope()`**: Leave the scope, discard all bindings in it
- **`define(name, value)`**: Add to current (innermost) scope
- **`get(name)`**: Search from innermost scope outward
- **`assign(name, value)`**: Find existing binding, replace its value
- **`capture()`**: Clone all visible bindings (for closures)

This is a standard lexical scoping implementation. Variable lookup starts at
the innermost scope and walks outward until it finds a match.


## How execution works

### Phase 1: Registration (`register.rs`)

Before executing anything, `run()` walks all declarations and stores them:

- Functions → `self.functions`
- Structs → `self.struct_decls`
- Enums → `self.enums`
- `extend` block methods → `self.methods[type_name][method_name]`
- Imports → register modules (fs, io, time, etc.) as values

Then it calls the `main()` function (or runs tests/benchmarks).


### Phase 2: Expression evaluation (`eval_expr.rs`)

`eval_expr(expr)` takes an AST expression and returns a `Value`. It's a big
match on `ExprKind`:

```rust
fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeDiagnostic> {
    match &expr.kind {
        ExprKind::Int(n, _) => Ok(Value::Int(*n)),
        ExprKind::String(s) => {
            // Handle string interpolation: "hello {name}"
            if s.contains('{') {
                let interpolated = self.interpolate_string(s)?;
                Ok(Value::String(Arc::new(Mutex::new(interpolated))))
            } else {
                Ok(Value::String(Arc::new(Mutex::new(s.clone()))))
            }
        }
        ExprKind::Ident(name) => {
            // Look up variable, or function, or type constructor
            if let Some(val) = self.env.get(name) { return Ok(val.clone()); }
            if self.functions.contains_key(name) { return Ok(Value::Function { .. }); }
            // Check for Vec, Map, string, Pool, Channel, etc.
            match base_name {
                "Vec" => Ok(Value::TypeConstructor { kind: TypeConstructorKind::Vec, .. }),
                // ...
            }
        }
        ExprKind::Call { func, args } => self.eval_call(func, args),
        ExprKind::MethodCall { object, method, args } => {
            self.eval_method_call(object, method, args)
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            let cond_val = self.eval_expr(cond)?;
            if cond_val.is_truthy() {
                self.eval_block(then_branch)
            } else if let Some(else_b) = else_branch {
                self.eval_block(else_b)
            } else {
                Ok(Value::Unit)
            }
        }
        // ... ~30 more arms
    }
}
```


### Phase 3: Statement execution (`exec_stmt.rs`)

`exec_stmt(stmt)` runs a statement for its side effects:

- **Const/Let**: Evaluate the initializer, call `env.define(name, value)`
- **Assignment**: Evaluate the right side, call `env.assign(target, value)`.
  For field paths like `point.x = 5`, walks into the struct value.
- **Return**: Evaluate the expression, return it as a special signal that
  unwinds the call stack.
- **For loop**: Evaluate the iterator, loop: bind current element, execute body
- **While loop**: Loop: evaluate condition, if truthy execute body
- **Ensure**: Register cleanup code that runs when the function returns


### Phase 4: Function calls (`call.rs`, `dispatch.rs`)

When the interpreter hits a function call:

1. Evaluate all arguments
2. Push a new scope
3. Bind each argument to its parameter name
4. Execute the function body
5. Pop the scope
6. Return the result

For method calls, dispatch order is:

1. **Built-in methods** (`builtins/`): Hardcoded implementations on primitive
   types and collections. `i32.add()`, `string.len()`, `Vec.push()`, etc.
2. **User-defined methods**: Look up in `self.methods[type_name][method_name]`
   from `extend` blocks.


## Type constructors and static methods

`Vec.new()`, `Map.new()`, `string.new()` work through `TypeConstructor` values.
When the interpreter sees `Vec` as an identifier, it returns
`Value::TypeConstructor { kind: TypeConstructorKind::Vec }`. When `.new()` is
called on it, `dispatch.rs` creates a new empty `Vec` (or whatever type).

This is how "static methods" work without a special static method concept—the
type name itself is a value.


## Stdlib modules

`fs`, `io`, `net`, `time`, etc. are represented as `Value::Module(ModuleKind)`.
When you write `import fs`, the resolver registers `fs` in scope, and the
interpreter creates a `Module(ModuleKind::Fs)` value. Then `fs.read_file(path)`
dispatches to the Rust implementation in `stdlib/fs.rs`.

Each module file implements the actual operations using Rust's standard library:
- `fs.rs` → `std::fs::read_to_string`, `std::fs::write`
- `net.rs` → `std::net::TcpListener`, `std::net::TcpStream`
- `time.rs` → `std::time::Instant`, `std::thread::sleep`
- `json.rs` → hand-written recursive descent JSON parser


## Concurrency in the interpreter

The interpreter supports real concurrency:

- **`spawn(|| { ... })`** creates an OS thread (via `std::thread::spawn`),
  captures the closure's environment, runs the body.
- **Channels**: `Channel.new()` creates an `mpsc` channel pair. `.send()` and
  `.recv()` work across threads.
- **`Shared<T>`**: `Arc<RwLock<Value>>` for thread-safe shared state.
- **Mutex, Atomic**: Thin wrappers around Rust's `std::sync` primitives.

The interpreter's `Value` types use `Arc<Mutex<...>>` pervasively, which makes
them safe to share across threads. This is more expensive than single-threaded
operation but means concurrency "just works."


## Testing support

`rask test` uses the same interpreter but:

1. Collects all `test` declarations instead of calling `main()`
2. For each test, creates a fresh interpreter with a captured output buffer
3. Runs the test body, catches panics/errors
4. Reports pass/fail with timing

`rask benchmark` is similar but runs each benchmark body repeatedly (target:
100 iterations or 1 second) and reports min/max/mean/median statistics.
