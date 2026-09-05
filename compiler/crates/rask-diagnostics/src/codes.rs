// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Error code registry.
//!
//! Maps error codes (E0001, E0308, etc.) to titles, categories, and explanations.
//! Used by `rask explain <code>` and for error display.

use std::collections::HashMap;

/// Registry of all known error codes.
pub struct ErrorCodeRegistry {
    codes: HashMap<&'static str, ErrorCodeInfo>,
}

/// Information about a single error code.
pub struct ErrorCodeInfo {
    pub code: &'static str,
    pub title: &'static str,
    pub category: ErrorCategory,
    pub description: &'static str,
    pub example: &'static str,
}

/// Error category for grouping.
#[derive(Debug, Clone, Copy)]
pub enum ErrorCategory {
    Syntax,
    Resolution,
    Type,
    Trait,
    Ownership,
    /// The `R00xx` namespace: something that went wrong while the program ran,
    /// not while it was compiled. A reader looking one up off a panic has the
    /// same question as one reading a compile error off a build (#992).
    Runtime,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCategory::Syntax => write!(f, "Syntax"),
            ErrorCategory::Resolution => write!(f, "Resolution"),
            ErrorCategory::Runtime => write!(f, "Runtime"),
            ErrorCategory::Type => write!(f, "Type"),
            ErrorCategory::Trait => write!(f, "Trait"),
            ErrorCategory::Ownership => write!(f, "Ownership"),
        }
    }
}

macro_rules! register_codes {
    ($($code:literal => ($title:literal, $cat:expr, $desc:literal, $example:literal)),* $(,)?) => {{
        let mut map = HashMap::new();
        $(
            map.insert($code, ErrorCodeInfo {
                code: $code,
                title: $title,
                category: $cat,
                description: $desc,
                example: $example,
            });
        )*
        map
    }};
}

impl Default for ErrorCodeRegistry {
    fn default() -> Self {
        use ErrorCategory::*;

        Self {
            codes: register_codes! {
                // Lexer errors (E00xx)
                "E0001" => ("unexpected character", Syntax,
                    "The lexer encountered a character that isn't valid in Rask source code. This usually means a stray special character or encoding issue.",
                    "// Error: unexpected '@' (outside attribute position)\nlet x = @value"),
                "E0002" => ("unterminated string literal", Syntax,
                    "A string was opened with `\"` but never closed. Every string literal must have a matching closing quote on the same line.",
                    "// Error: string never closed\nlet msg = \"hello world"),
                "E0003" => ("invalid escape sequence", Syntax,
                    "A backslash in a string was followed by a character that isn't a recognized escape. Valid escapes: \\n, \\t, \\r, \\\\, \\\", \\0.",
                    "// Error: \\q is not a valid escape\nlet s = \"path\\qname\""),
                "E0004" => ("invalid number format", Syntax,
                    "A numeric literal has an invalid format — perhaps a suffix typo, multiple dots, or an invalid digit for the base.",
                    "// Error: invalid suffix\nlet x = 42i3  // did you mean i32?"),

                // Parser errors (E01xx)
                "E0100" => ("unexpected token", Syntax,
                    "The parser encountered a token that doesn't make sense in the current context. Check for missing operators, mismatched brackets, or Rust syntax habits.",
                    "// Error: unexpected '::'\nlet x = Option::Some(1)  // use Option.Some(1)"),
                "E0101" => ("expected token not found", Syntax,
                    "The parser expected a specific token (like a closing bracket or keyword) but found something else.",
                    "// Error: expected '}'\nfunc main() {\n    println(\"hello\")\n// missing closing brace"),
                "E0102" => ("invalid syntax", Syntax,
                    "The source code doesn't match any valid Rask construct. Common causes: using Rust syntax, missing keywords, or incorrect statement structure.",
                    "// Error: use 'func' not 'fn'\nfn add(a: i32, b: i32) -> i32 { return a + b }"),

                // Resolver errors (E02xx)
                "E0200" => ("undefined symbol", Resolution,
                    "A name was used that hasn't been defined in the current scope. Check spelling, imports, and that the definition appears before use.",
                    "func main() {\n    println(value)  // error: 'value' not defined\n}"),
                "E0201" => ("duplicate definition", Resolution,
                    "Two items share the same name in the same scope. Rename one of them.",
                    "func add(a: i32) -> i32 { return a }\nfunc add(a: i32) -> i32 { return a }  // error: duplicate"),
                "E0202" => ("circular dependency", Resolution,
                    "Two or more modules depend on each other in a cycle. Break the cycle by extracting shared types into a separate module.",
                    "// a.rk imports b, b.rk imports a → cycle"),
                "E0203" => ("symbol not visible", Resolution,
                    "The symbol exists but isn't accessible from the current module. Only `public` items are visible outside their defining module.",
                    "// In module A:\nfunc helper() { }  // not public\n// In module B:\nA.helper()  // error: not visible"),
                "E0204" => ("break outside of loop", Resolution,
                    "`break` (or `deliver`) can only appear inside a loop body (while, for, loop). It cannot be used in functions or top-level code.",
                    "func main() {\n    break  // error: not in a loop\n}"),
                "E0205" => ("continue outside of loop", Resolution,
                    "`continue` can only appear inside a loop body. It skips to the next iteration.",
                    "func main() {\n    continue  // error: not in a loop\n}"),
                "E0206" => ("return outside of function", Resolution,
                    "`return` can only appear inside a function body. It cannot be used at the top level.",
                    "return 42  // error: not in a function"),
                "E0207" => ("unknown package", Resolution,
                    "An import references a package that can't be found. Check the package name and that it's listed as a dependency.",
                    "import unknown_pkg  // error: package not found"),
                "E0208" => ("shadows import", Resolution,
                    "A local definition has the same name as an imported symbol. This can cause confusion. Rename the local or use an import alias.",
                    "import math\nlet math = 42  // error: shadows import"),
                "E0209" => ("shadows built-in", Resolution,
                    "A definition has the same name as a built-in type or function. This can cause confusing errors later. Choose a different name.",
                    "struct Vec { }  // error: shadows built-in Vec"),
                "E0210" => ("name is not in scope — it needs an import", Resolution,
                    "Nothing in Rask comes pre-imported. A stdlib name is in scope where the program asked for it and nowhere else, which is also what leaves the name free for a program that wants it for something of its own (structure.modules/IM1).\n\nThe import can name the module (`import math`, then `math.sin`) or the member (`import math.sin`, then `sin`) — the second is for names used often enough that the qualifier is noise.",
                    "let d = Duration.seconds(3)   // error: `Duration` is not in scope\n// fix: ask for it\nimport time.Duration\nlet d = Duration.seconds(3)"),

                // Type errors (E03xx)
                "E0308" => ("mismatched types", Type,
                    "An expression has a different type than what was expected. This is the most common type error — check that return types, assignments, and function arguments match.",
                    "func double(x: i32) -> i32 {\n    return \"hello\"  // error: expected i32, found string\n}"),
                "E0309" => ("undefined type", Type,
                    "A type annotation references a type that doesn't exist. Check spelling and imports.",
                    "func f(x: Strng) { }  // error: did you mean 'string'?"),
                "E0310" => ("arity mismatch", Type,
                    "A function was called with the wrong number of arguments. Check the function signature.",
                    "func add(a: i32, b: i32) -> i32 { return a + b }\nadd(1)  // error: expected 2 args, found 1"),
                "E0311" => ("type is not callable", Type,
                    "You tried to call something that isn't a function or closure. Only functions, closures, and constructors support `()` syntax.",
                    "let x = 42\nx()  // error: i64 is not callable"),
                "E0312" => ("no such field", Type,
                    "The struct doesn't have a field with this name. Check the struct definition for available fields.",
                    "struct Point { x: i32, y: i32 }\nlet p = Point { x: 1, y: 2 }\np.z  // error: no field 'z' on Point"),
                "E0313" => ("no such method", Type,
                    "The type doesn't have a method with this name. Check the extend blocks for available methods, or verify the receiver type.",
                    "let v = Vec.new()\nv.length()  // error: did you mean v.len()?"),
                "E0314" => ("infinite type", Type,
                    "A type would need to contain itself without indirection, creating an infinite-size type. Use `Heap<T>` for indirection.",
                    "struct Node {\n    next: Node  // error: infinite size\n    // fix: next: Heap<Node>?\n}"),
                "E0315" => ("cannot infer type", Type,
                    "The compiler can't determine a type from context alone. Add an explicit type annotation.",
                    "let x = Vec.new()  // error: Vec of what?\n// fix: let x: Vec<i32> = Vec.new()"),
                "E0316" => ("invalid try context", Type,
                    "`try` propagates errors to the caller, so the enclosing function must return `T or E` (a Result type).",
                    "func f() {\n    let x = try might_fail()  // error: f() doesn't return Result\n}"),
                "E0317" => ("try outside function", Type,
                    "`try` can only appear inside a function body. It needs a function return type to propagate errors to.",
                    "let x = try some_call()  // error: not in a function"),
                "E0318" => ("missing return statement", Type,
                    "A function with a non-void return type doesn't return a value on all paths. Rask requires explicit `return` in functions.",
                    "func double(x: i32) -> i32 {\n    x * 2  // error: missing 'return'\n    // fix: return x * 2\n}"),
                "E0319" => ("generic argument error", Type,
                    "A generic type was instantiated with the wrong number or kind of arguments.",
                    "// Vec takes 1 type param\nlet x: Vec<i32, string> = Vec.new()  // error"),
                "E0320" => ("aliasing violation", Type,
                    "A value was mutated while it's being borrowed. Finish using the borrow before mutating, or clone the value.",
                    "let v = Vec.new()\nlet first = v[0]  // borrows v\nv.push(4)  // error: v is borrowed"),
                "E0321" => ("mutate read-only parameter", Type,
                    "Parameters are read-only by default in Rask. To modify a parameter, add the `mutate` keyword.",
                    "func reset(v: Vec<i32>) {\n    v.clear()  // error: v is read-only\n}\n// fix: func reset(mutate v: Vec<i32>)"),
                "E0322" => ("volatile view stored", Type,
                    "A view (reference) into a growable collection was stored across a statement boundary. Views into Vec, Pool, and Map are instant — they're released at the semicolon.",
                    "let v = Vec.new()\nlet elem = v[0]  // view into v\n// elem is invalid after this line if v changes"),
                "E0323" => ("mutate while viewed", Type,
                    "A collection was mutated while a view into it exists. This could invalidate the view. Finish using the view first.",
                    "let v = Vec.new()\nlet elem = v[0]\nv.push(4)  // error: v viewed by elem"),
                "E0324" => ("heap allocation in @no_alloc function", Type,
                    "@no_alloc functions run in real-time contexts where heap allocation causes unpredictable latency. Use stack-allocated alternatives or pre-allocated buffers.",
                    "@no_alloc\nfunc process(data: [f32; 64]) {\n    let v = Vec.new()  // error: allocates\n}"),
                "E0325" => ("write in frozen pool context", Type,
                    "A `using frozen Pool<T>` context is read-only (mem.pools/PF5): no writes through handles, and no insert/remove/clear. Drop `frozen` if the function needs to mutate the pool.",
                    "func heal(h: Handle<Player>) using frozen Pool<Player> {\n    h.health += 10  // error: frozen context\n}\n// fix: using Pool<Player>  (drop `frozen`)"),
                "E0327" => ("required `Link<T>` edge unsupported", Type,
                    "A required edge (`Link<T>` with no `?`) needs a batch to construct — a cycle needs one side written before its target exists — and a declared delete policy (cascade or restrict) for when its target dies, because there is no `none` to set it to. Neither is implemented yet, so write `Link<T>?`. Inside a container a bare link is fine, because delete drops the entry rather than nulling it.",
                    "struct Entity {\n    target: Link<Entity>     // error: no `none` to fall back to\n    target2: Link<Entity>?   // fix\n    children: Vec<Link<Entity>>  // fine: delete drops the entry\n}"),
                "E0328" => ("use after delete on a `Link<T>`", Type,
                    "A `Link<T>` is a pointer to a node, and `rack.delete` frees that node — so every name for it dies at once. This is a use after free, proven at compile time. A link held in a `Link<T>?` *field* survives a delete, because the rack nulls it and the `?` makes you check; a local can't be reached by the rack, so the compiler tracks it instead. Note the invalidation point is the `delete` you wrote, not an inferred last use — the analysis stays inside one function body.",
                    "let n = rack.insert(Node { name: \"a\" })\nrack.delete(n)\nprintln(\"{n.name}\")   // error: `n` names a deleted node\n\n// fix: read before deleting\nlet label = n.name\nrack.delete(n)\nprintln(\"{label}\")"),
                "E0378" => ("node written through a rack you may only read", Type,
                    "A `Link<T>` is a path to a node, not permission to change it. The node lives in a rack, so the rack is what says whether it may be written — the same rule `Handle` has always had, where `scene.nodes[h].f = x` needs `mutate scene`. Asking the rack rather than the link is also what makes a read-only graph free: a function taking `s: Rack<T>` can read every node and write none, and because the question is about the rack, following an edge doesn\'t change the answer — there is nothing to propagate and no way to launder a readable link into a writable one.",
                    "func combat(world: Rack<Entity>) {\n    for e in world.nodes() {\n        if e.target? as t { t.health -= e.damage }   // error: `world` is only readable\n    }\n}\n\n// fix: say the nodes get written\nfunc combat(mutate world: Rack<Entity>) {\n    for e in world.nodes() {\n        if e.target? as t { t.health -= e.damage }\n    }\n}"),
                "E0331" => ("mutation method on `string`", Type,
                    "`string` is immutable (std.strings/S7). It's a 16-byte value and every copy shares one buffer, so an in-place write would change copies you never touched. `StringBuilder` is the mutable one: it owns its buffer alone, and `build()` transfers it to a string without copying.",
                    "mut out = \"\"\nout.push_str(\"hello\")   // error: string has no push_str\n\n// fix:\nmut b = StringBuilder.new()\nb.push(\"hello\")\nb.push_char('!')\nlet out = b.build()"),
                "E0342" => ("unknown context", Type,
                    "A `using` block references a context that doesn't exist. Valid contexts are `Multitasking` and `ThreadPool`.",
                    "using Foo {\n    // error: unknown context `Foo`\n}"),
                "E0345" => ("type name called as a function", Type,
                    "`Name(value)` is the constructor for a nominal type declared with `type Name = Underlying` (T7). Structs have named fields and no tuple form (S1), so they're built with a literal; enums are named by variant.",
                    "struct TaskId { public value: u64 }\n\nlet a = TaskId(1)             // error\nlet b = TaskId { value: 1 }   // fix"),
                "E0346" => ("task-local Shared sent to another task", Type,
                    "`Shared<T, Local>` takes no lock, so it may only be reached from the task that made it. Sending one to another task would be a data race, so it's rejected. `Local` is the opt-out; this rule is what makes it safe to reach for (conc.sync/SH7).",
                    "let counter = Shared.local(0)\nlet h = spawn(|| {\n    with counter.write() as c { c += 1 }   // error\n})\n\n// fix: a strategy that locks\nlet counter = Shared.new(0)"),
                "E0380" => ("retired box type", Type,
                    "`Cell` and `Mutex` were separate box types. They were one concept with `Shared` — one value, several accessors, a scoped view — differing only in what synchronization they took, so they became strategies on `Shared<T, S>` (analysis.storage-consolidation).",
                    "let c = Cell.new(0)          // error\nlet m = Mutex.new(0)         // error\n\n// fix:\nlet c = Shared.local(0)      // no lock, one task\nlet m = Shared.mutex(0)      // a plain lock"),
                "E0381" => ("Shared strategy mismatch", Type,
                    "The strategy in `Shared<T, S>` decides which lock the accessors take, so the constructor and the annotation have to agree. A bare `Shared<T>` means `Shared<T, Readers>` (conc.sync/SH2) — it is a defaulted parameter, not an absent one. Code that works under any strategy says so: `func serve<S>(c: Shared<Config, S>)` (SH4).",
                    "func bump(c: Shared<i32>) -> i32 {      // Shared<i32, Readers>\n    with c.write() as v { return v + 1 }\n}\n\nlet c = Shared.local(0)                 // Shared<i32, Local>\nbump(c)                                 // error\n\n// fix: agree on one, or be generic over it\nfunc bump<S>(c: Shared<i32, S>) -> i32 { ... }"),
                "E0351" => ("runtime context on signature", Type,
                    "`using Multitasking` and `using ThreadPool` install a process-global runtime slot. They cannot appear on function signatures — only on block expressions.",
                    "// Error: signature-level using is not allowed\nfunc run_tasks() using Multitasking { }\n\n// Fix: wrap the call site instead\nfunc main() {\n    using Multitasking {\n        run_tasks()\n    }\n}"),
                "E0352" => ("`spawn` with no Multitasking scope", Type,
                    "`spawn` submits the task to the runtime that `using Multitasking { }` installs, and nothing here installs one — so the program would panic on its first task. Only a function nothing calls is blamed at the `spawn` itself: the entry point, a `test` block, a `@test` function (conc.async/CC1, std.testing/T17). Anywhere else the block belongs at the call site rather than the definition, which is what lets a library function spawn per request and leave the scope to whoever calls it; that case is E0353.",
                    "func main() {\n    let h = spawn(|| { work() })   // error: no scope installs a runtime\n    h.join()\n}\n\n// fix: open the scope around the spawn and its join\nfunc main() {\n    using Multitasking {\n        let h = spawn(|| { work() })\n        h.join()\n    }\n}"),
                "E0353" => ("call reaches `spawn` with no Multitasking scope", Type,
                    "This function reaches `spawn` through its call graph, and no `using Multitasking { }` block encloses the call — so the task has no runtime to go to and the program panics on reaching it. Rask infers which functions need a runtime rather than marking them in signatures, so nothing on the callee's declaration says this: the error is at the call, and names the function that spawns (conc.async/CC2). Adding a `spawn` deep in a private helper can therefore give callers a new scope requirement, the same way any API change would.",
                    "func launch_worker() {\n    spawn(|| { work() }).detach()   // fine here — the caller opens the scope\n}\n\nfunc main() {\n    launch_worker()                 // error: reaches `spawn`, no scope\n}\n\n// fix: open it at the call\nfunc main() {\n    using Multitasking { launch_worker() }\n}"),
                "E0354" => ("duplicate variant in sum type", Type,
                    "A sum type cannot contain the same payload variant twice — `(T or E) or E` is ambiguous, because the compiler picks the branch from the value's type and an `E` value fits both. `none` is exempt: `T??` is a legal two-layer optional whose layers stay distinct. Use a named enum if you need two flavours of the same error.",
                    "// Error: duplicate `ParseError` branch\nfunc f() -> (i32 or ParseError) or ParseError { }\n\n// Fix: use a named enum\nenum LookupResult { Found(User), Missing, Forbidden }\nlet x: LookupResult = LookupResult.Missing"),
                "E0358" => ("generic instantiation collapses `T or E`", Type,
                    "A generic returning `T or E` was called with a type argument equal to `E`. Both branches would then carry the same type and the caller could not tell a success from an error. The signature's `T or E` is itself the requirement that they stay distinct — it is checked here, at the call, where the type argument is known. Newtype one side to keep them apart.",
                    "enum CacheError { Miss }\n\nfunc cached<T>(v: T) -> T or CacheError { return v }\n\nfunc main() {\n    let ok = cached(42)                 // T = i32: fine\n    let bad = cached(CacheError.Miss)   // error: T = CacheError\n\n    // Fix: newtype the success side\n    type Cached = CacheError with (…)\n}"),
                "E0356" => ("unknown type in signature", Type,
                    "A PascalCase name in a function signature doesn't resolve to any declared type. Only single uppercase letters (T, U, K, V) are auto-generic type parameters — longer names must be declared types. This catches typos early instead of silently treating them as generics.",
                    "struct Config { port: i32 }\nfunc load(c: Confg) { }  // error: did you mean `Config`?\n\n// Auto-generic still works with single letters:\nfunc swap(a: T, b: T) -> (T, T) { return (b, a) }"),
                "E0357" => ("single-letter type name", Type,
                    "Single uppercase letters are reserved for type parameters. A struct, enum, trait, or union named `T` would be shadowed by the type-parameter convention in every signature.",
                    "struct T { }  // error: reserved for type parameters\n// fix: struct Token { }"),
                "E0355" => ("error type mismatch in try", Type,
                    "`try` propagates the inner error to the enclosing function, so the two error types have to line up. They line up three ways: the same type, a member of the function's error union, or a single-payload variant of the function's error enum. Anything else needs the wrap named at the call — `expr catch e => return …`.",
                    "enum ParseError { Syntax(string) }\nenum ApiError { Parse(ParseError), BadRequest(string) }\n\nfunc inner() -> i32 or ParseError { return 42 }\n\nfunc outer() -> i32 or ApiError {\n    let x = try inner()  // ok: ApiError.Parse takes a single ParseError\n    return x\n}"),
                "E0359" => ("ambiguous error wrap in try", Type,
                    "Two or more variants of the function's error enum take the propagated error as their only payload, so `try` has no way to choose. Name the variant at the call site.",
                    "enum StoreError { NotFound(string) }\nenum ApiError { Store(StoreError), Fatal(StoreError) }\n\nfunc outer() -> i32 or ApiError {\n    let x = try lookup()  // error: Store or Fatal?\n\n    // Fix: say which\n    let y = lookup() catch e => return ApiError.Store(e)\n    return y\n}"),
                "E0361" => ("type could not be inferred", Type,
                    "Inference finished with this binding's type still open. Usually nothing in scope constrains it — a `Vec.new()` that never gets pushed to, a const whose initializer the compiler can't evaluate. Writing the type out settles it. If it looks like it should have been inferable, that's a compiler bug: the alternative to this error is guessing a size, which silently corrupts floats, strings and structs.",
                    "let v = Vec.new()          // error: couldn't work out the type of `v`\nlet v: Vec<i64> = Vec.new()  // fix"),
                "E0364" => ("`??` on a result", Type,
                    "`??` fills in a missing value, and only an optional (`T?`) can be missing. A `T or E` carries an error instead, and dropping it silently is what `??` would do here — `catch` says out loud that an error is being discarded.",
                    "let n = parse(text) ?? 0        // error: parse returns `i64 or ParseError`\nlet n = parse(text) catch _ => 0  // fix: the discard is visible"),
                "E0360" => ("mutate through shared read lock", Type,
                    "`.read()` takes a shared lock — other readers may hold it at the same time, so its binding is read-only and never writes back. Use `.write()` for exclusive access when you need to mutate.",
                    "let config = Shared.new(Config {})\nwith config.read() as c {\n    c.timeout = 10   // error: read lock\n}\n// fix:\nwith config.write() as c {\n    c.timeout = 10   // exclusive — written back at block exit\n}"),

                // Trait errors (E07xx)
                "E0700" => ("trait bound not satisfied", Trait,
                    "A generic function requires a trait bound that the provided type doesn't implement.",
                    "func print_it<T: Display>(x: T) { }\nprint_it(MyStruct {})  // error: MyStruct doesn't implement Display"),
                "E0701" => ("missing trait method", Trait,
                    "An `extend` block claims to implement a trait but doesn't define all required methods.",
                    "trait Printable { func show(self) }\nextend Point as Printable { }  // error: missing show()"),
                "E0702" => ("method signature mismatch", Trait,
                    "A trait method implementation has a different signature than the trait declaration.",
                    "trait Add { func add(self, other: Self) -> Self }\nextend Point as Add {\n    func add(self) -> Self { }  // error: wrong params\n}"),
                "E0703" => ("unknown trait", Trait,
                    "A trait name was used that doesn't exist. Check spelling and imports.",
                    "extend Point as Printabel { }  // error: did you mean Printable?"),
                "E0704" => ("conflicting trait methods", Trait,
                    "Two trait implementations provide the same method name for a type. This creates ambiguity.",
                    "// Both TraitA and TraitB define show()"),

                // Ownership errors (E08xx)
                "E0800" => ("use after move", Ownership,
                    "A value was used after being moved. Once ownership transfers, the original binding is invalid. Clone if you need both.",
                    "let v = Vec.new()\ntake_ownership(own v)\nv.len()  // error: v was moved"),
                "E0801" => ("borrow conflict", Ownership,
                    "Multiple borrows conflict — typically a mutable borrow while an immutable borrow exists.",
                    "let v = Vec.new()\nlet first = v[0]  // immutable borrow\nv.push(4)  // error: mutable borrow conflicts"),
                "E0802" => ("mutate while borrowed", Ownership,
                    "A value was mutated while it's borrowed. The borrow must end before mutation is allowed.",
                    "let s = \"hello\"\nlet r = s\ns = \"world\"  // error: s is borrowed by r"),
                "E0803" => ("instant borrow escapes", Ownership,
                    "A reference from a collection access was stored past its valid scope. Collection references are instant — valid for one expression only.",
                    "let v = Vec.new()\nlet elem = v[0]\n// elem may be invalid if v reallocates"),
                "E0804" => ("borrow escapes scope", Ownership,
                    "A reference outlives the value it borrows from. The borrowed value must live at least as long as the reference.",
                    "func bad() -> string {\n    let local = \"temp\"\n    return local  // error if local is stack-allocated\n}"),
                "E0805" => ("resource not consumed", Ownership,
                    "A resource-typed value (marked with @resource) wasn't properly consumed. Resource types must be explicitly closed, released, or passed to a consuming function.",
                    "func open_file() {\n    let f = fs.open(\"data.txt\")\n    // error: f not consumed (must call f.close())\n}"),
                "E0806" => ("move from borrowed parameter", Ownership,
                    "A borrowed parameter was used in a context that requires ownership. Parameters are borrowed by default — the caller retains ownership. Use `take` to transfer ownership into the function.",
                    "func push(self, value: T) {\n    self.data[i] = value  // error: value is borrowed\n}\n// fix: func push(self, take value: T)"),
                "E0811" => ("use after discard", Ownership,
                    "`discard` explicitly drops a value and invalidates its binding. Using the binding after `discard` is a compile error (D1).",
                    "let data = load_data()\ndiscard data\nprintln(data)  // error: use of discarded value"),
                "E0812" => ("discard resource type", Ownership,
                    "Resource types (@resource) must be consumed properly via their consuming method (.close(), .release(), etc). `discard` on a resource type is a compile error (D3).",
                    "@resource\nstruct File { fd: i32 }\nlet f = File { fd: 1 }\ndiscard f  // error: use f.close() instead"),
                "E0813" => ("use after maybe-move", Ownership,
                    "A value moved on some paths but not all (e.g. one `if` branch) was used after the paths merged. The spec treats maybe-moved as moved (O3) — move on every path, or keep the use inside the branch that still owns the value.",
                    "let v = Vec.new()\nif c { take(own v) }\nv.len()  // error: v may have been moved"),
                "E0817" => ("invalid `as` cast", Type,
                    "`as` permits only lossless widening (CV1). Narrowing, sign reinterpretation, float↔int, int→char, and int↔bool are compile errors — name a policy with one of the six conversion methods (`to`, `wrap`, `clamp`, `round`, `floor`, `ceil`) or with `char.from_u32`.",
                    "let x: i8 = big as i8  // error: use `big.to<i8>()!`, `big.wrap<i8>()` or `big.clamp<i8>()`"),
                "E0818" => ("invalid conversion form", Type,
                    "A conversion method was used where its policy means nothing (CV11–CV16). Each is defined only on the source and target kinds it can say something about: `wrap` and `clamp` are integer-to-integer, `floor` and `ceil` are float-to-integer, and `round` is anything but integer-to-integer — there is nothing to round there. Anywhere else is an error rather than a no-op, so a method that reads as if it did something always did.",
                    "let x = n.floor<i32>()   // error if n is already an integer\n// fix: `n.to<i32>()!`, `n.wrap<i32>()` or `n.clamp<i32>()`"),
                "E0819" => ("index type mismatch", Type,
                    "An index expression `c[i]` used the wrong index type. Vec, arrays, slices, and strings are position-indexed by an integer; `Map<K,V>` is indexed by `K`; `Pool<T>` is indexed by `Handle<T>`. Range indexing (slicing) only works on Vec, arrays, slices, and strings.",
                    "let s = \"hi\"\nv[s]  // error: index a Vec with an integer, not a string"),
                "E0820" => ("linear value in container", Ownership,
                    "A Vec or Map element (or Map key) is a linear value — an @resource type, a transitively-linear struct/enum, or an optional/tuple/array built from one. Vec/Map drop can't consume linear elements, so they'd be silently dropped (RC1/RC3). Use `Pool<T>` (explicit removal, RC2) or `T?` (match and consume, RC4).",
                    "@resource\nstruct File { fd: i32 }\nlet files: Vec<File> = Vec.new()  // error: use Pool<File>"),
                "E0821" => ("ensure receiver maybe-consumed", Ownership,
                    "A resource with a pending `ensure` was consumed on some paths but not all, and the paths merge before scope exit (C4). Which cleanup runs must be statically definite — never decided by hidden runtime state (C3). Exit inside the consuming branch, or consume on every path.",
                    "let tx = try db.begin()\nensure tx.rollback()\nif fast { tx.commit() }  // error: paths merge with tx maybe-consumed\nlog(\"done\")"),
                "E0822" => ("missing struct field", Type,
                    "A struct literal left out a field that has no default value. Construction never zero-initializes — a defaultless field must be given a value, or declared with a default (`field: T = value`). A spread (`..base`) supplies every unlisted field.",
                    "struct Config { host: string, port: i32 = 8080 }\nlet c = Config {}  // error: missing field `host`"),
                "E0823" => ("method name shared by two types", Type,
                    "Two different types have the same name — usually a program type and a stdlib one — and both define this method. Compiled functions are identified by `Type_method`, so the two methods want the same name and only one can have it. The type the name currently refers to gets it; a call needing the other has nowhere to go. Rename one of the types.",
                    "// stdlib already has `enum JsonError` with `message()`\nstruct JsonError { detail: string }\nextend JsonError {\n    func message(self) -> string { return self.detail }\n}\n// fix: name it something else, e.g. `ConfigJsonError`"),
                "E0824" => ("public duck trait", Type,
                    "A `duck trait` was declared `public`. Duck traits match by shape instead of by declaration, which makes them a versioning trap across a package boundary: an external type could start or stop satisfying the trait because its author added or removed a method, with nothing in either diff to notice. So duck traits stay package-internal (type.generics/DT1) — they're for code you're still sketching. Drop `duck` to harden the trait (the compiler generates the conformance declarations for types that already match), or drop `public`.",
                    "public duck trait Frobber {\n    func frobnicate(self)\n}\n// fix: `public trait Frobber` (nominal), or `duck trait Frobber` (package-internal)"),
                "E0825" => ("integer literal out of range", Type,
                    "An integer literal doesn't fit the type it ended up with. Unsuffixed literals are `i32` by default (type.primitives/L1) and widen to `i64` when the value needs it; a literal that reaches a narrower type through an annotation, a suffix, or a parameter has to fit that type. Nothing wraps silently — pick a wider type, or convert at the use site.",
                    "let b: u8 = 300  // error: 300 doesn't fit u8 (0..=255)\n// fix: `let b: u16 = 300`, or `let b = (300 as i64).wrap<u8>()`"),
                "E0826" => ("type does not implement Displayable", Type,
                    "`{}` in a format template calls `to_string()`, which comes from `Displayable` (std.fmt/D4). Primitives have it; structs and enums opt in with `extend Type with Displayable`, and error types get it for free from `message()` (D5). Optionals and results are never Displayable — an optional may have nothing to show, so the missing case has to be spelled out at the call.",
                    "let found: User? = lookup(id)\nprintln(\"{found}\")   // error: `User?` has no to_string()\n// fix: `println(\"{found ?? \\\"nobody\\\"}\")`, or narrow first with `if found? as u { … }`"),
                "E0827" => ("type can't be iterated", Type,
                    "A `for` loop walks a Vec, Map, Pool, array, slice, range or iterator chain. The thing in the iterator position resolved to a single value instead — most often a count where the range was meant, a string where `.chars()` was meant, or a struct where one of its collection fields was meant. A container reached through a field resolves later than the loop, so this is reported once its type settles rather than at the loop itself.",
                    "for x in self.count { … }   // error: `i64` can't be iterated\n// fix: `for x in 0..self.count { … }`"),
                "E0829" => ("with guard escapes its block", Type,
                    "A `with` block's guard — the name after `as` — is access to a box's payload for the block's duration, not a value of its own: boxes hand out no guards, so the inner value can't outlive its scope. Returning the bare guard identifier as the block's own produced value would hand back a live view into memory the lock no longer protects once the block ends. This only fires for struct/enum/union payloads; scalars and `string` copy out fine as-is, since a plain identifier read of those is already an independent value.",
                    "let c = with counter as g { g }   // error: `g` (a `Counter`) can't leave the block\n// fix: `with counter as g { g.hits }`, or a method that returns an owned `Counter`"),
                "E0838" => ("`as` to a target it can't convert to", Type,
                    "`as` has exactly two meanings: converting between numbers (CV1) and boxing a value as a trait object (`as any Trait`, TR5). To a collection, a struct, or an enum there is nothing left for it to mean but reinterpreting the bits, which is what `transmute` needs `unsafe` for. It used to be accepted silently with no check at all, so `[1, 2, 3] as Vec<i64>` lowered to a stack array whose address was handed to `Vec_len` — and indexing the result segfaulted from ordinary safe code. To give a value a type, annotate the binding; to reinterpret on purpose, say `unsafe`.",
                    "let v = [1, 2, 3] as Vec<i64>  // error
// fix: let v: Vec<i64> = [1, 2, 3]"),
                "E0837" => ("a `Heap` value was never dropped", Ownership,
                    "`Heap(…)` allocates, and the value has exactly one owner who has to consume it exactly once (mem.linear/L1) — otherwise the allocation is never freed. Consuming it means `drop(name)`, handing it to a `take` parameter, storing it in a field or enum payload, or returning it. `ensure` works too when the consumption happens on an error path. This is the same rule `@resource` follows; only the spelling of the fix differs.",
                    "let p = own Node { v: 7 }\nprintln(\"{p.v}\")      // error: `p` is never dropped\n// fix: add `drop(p)`"),
                "E0836" => ("a `mutate` parameter was left empty", Ownership,
                    "`mutate` is exclusive access, not ownership (mem.parameters/PM2): the caller keeps the value and goes on reading it after the call. That makes taking the value out and writing a replacement back legitimate — `out.push(b.build()); b = StringBuilder.new()` is exactly what the mode is for. Consuming it and putting nothing back is not: the caller reads a hole. A replacement has to be assigned on every path that reaches the return. If the function really does take the value for good, declare the parameter `take` instead — then the call site shows it going.",
                    "func drain(mutate b: StringBuilder) -> string {\n    return b.build()      // error: consumed, nothing put back\n}\n// fix: `func drain(take b: StringBuilder) -> string`"),
                "E0835" => ("cannot give away a borrowed parameter", Ownership,
                    "A parameter declared without `take` is the caller's value on loan (mem.parameters/PM1): they keep it and go on using it after the call. Handing it to a `take` parameter, an `own` argument, or a `take self` method would consume something the callee doesn't own — the caller is never told, and for a `@resource` that is a second close of a live handle. `mutate` is the same answer for a different reason: exclusive access lets you write through the parameter, not give it away. Put `take` on the declaration if the function really does consume its argument; then the call site shows the value going.",
                    "func handle(c: Conn) { c.close() }   // error: `close` takes ownership\n// fix: `func handle(take c: Conn) { c.close() }`"),
                "E0834" => ("type can't be a Map key", Type,
                    "A Map key has to be Hashable: equal keys must hash equal, or a key can be inserted and then never found again (type.generics/HA1). Auto-derive covers the primitives, a struct or enum whose every field or payload is itself Hashable, and a tuple or array of Hashable elements (type.tuples/TU11). Three things are left out — `f32`/`f64`, because `NaN != NaN` breaks the contract outright (HA4); an aggregate that reaches a float through one of its fields; and a nominal newtype, which inherits only the traits its `with (…)` clause names (type.aliases/T11).",
                    "type Id = u64\nlet ids: Map<Id, User> = Map.new()   // error: `Id` is not Hashable\n// fix: `type Id = u64 with (Equal, Hashable)`"),
                "E0833" => ("no trait by that name", Type,
                    "A bound, a conformance header, or an `as any Trait` cast named something that isn\'t a trait. Usually a typo or a missing import. This used to be reported as \"`_` does not implement X\" — an unknown trait has no type to blame, so the placeholder stood in for one and the message pointed at the type system instead of at the name (type.generics/G1).",
                    "func narrow<T: Intger>(x: i64) -> T { … }   // error: no trait named `Intger`\n// fix: `func narrow<T: Integer>(x: i64) -> T { … }`"),
                "E0832" => ("`!` on a value that is always there", Type,
                    "`x!` extracts the payload of a `T?` and panics when the value is absent (type.optionals/OPT13). On a left side that can never be absent there is no wrapper to extract from and nothing that could panic, so the operator has no meaning. This usually turns up after a refactor that made something non-optional and left the `!` behind. Drop it. Note `!` is also boolean negation, and on a `bool?` neither reading applies — that case is E0830.",
                    "let n: i64 = 5\nlet v = n!   // error: `i64` has no payload to force out\n// fix: `let v = n`"),
                "E0377" => ("excluded field has no default to decode from", Type,
                    "A decode has to build the whole struct, and a `private` or `@no_serialize` field never appears in the input — so its value comes from its declared default (type.structs/FD1, FD6) or from nowhere. With neither, the type isn\'t auto-`Decode` (std.encoding/E13a). Encoding is unaffected: it never needs a value for a field it omits, which is why the same type can be `Encode` and not `Decode`.",
                    "struct Config {\n    public host: string\n    @no_serialize\n    public token: string    // error: nothing to fill `token` on decode\n}\n// fix: `public token: string = \"\"`, or `@default(\"\")` for a decode-only default"),
                "E0376" => ("serialization annotation the compiler can\'t act on", Type,
                    "`@rename` takes a string literal and `@default` a comptime expression (std.encoding/E21), and both are checked at the declaration rather than at the encode site — an annotation the compiler silently ignores is worse than one it rejects, because the wire format then differs from what the source says. `@skip` is here too: it was renamed to `@no_serialize` because \"skip\" didn\'t say skip from what (E19), and left alone it reads as \"excluded\" while still serializing the field.",
                    "@skip\npublic token: string   // error: `@skip` is now `@no_serialize`\n\n@rename(user_name)\npublic name: string    // error: the serialized key has to be a string literal\n// fix: `@rename(\"user_name\")`"),
                "E0374" => ("`@small` type outgrew the copy threshold", Ownership,
                    "`@small` asserts one thing: the type stays within the 16-byte copy threshold (mem.value/SM1). It changes no semantics — what it buys is the *location* of the break. Without it, adding a field that pushes a struct past 16 bytes flips every assignment from copy to move, and those errors land wherever the type is used, with only the move note pointing back at the field nobody was looking at. With it, the error is at the declaration, before any call site sees it (mem.value/SM2). It composes with `@unique`: one is about layout, the other about copy semantics (SM4).",
                    "@small\nstruct Point { x: i64, y: i64, z: i64 }   // error: 24 bytes\n// fix: drop a field, shrink the fields (`x: i32`), or drop `@small`"),
                "E0375" => ("`@small` generic doesn\'t fit at this instantiation", Ownership,
                    "The fence is written at the definition but it is a promise about every instantiation, so it is checked per instantiation like any other generic bound (mem.value/SM3, type.generics/G2). The same source text can be 16 bytes at one type argument and 32 at another; only the second breaks the promise, and callers reading `@small` off the declaration have no way to tell which one they got. Rask has no size bound to narrow `T` with, so the two fixes are: don\'t instantiate it that wide, or drop the fence.",
                    "@small\nstruct Pair<T> { a: T, b: T }\nlet p = Pair<string> { a: \"x\", b: \"y\" }   // error: 32 bytes at `Pair<string>`\n// fix: `Pair<i64>` is 16 and fine — or drop `@small` from `Pair`"),
                "E0370" => ("integer doesn\'t fit its target", Type,
                    "An integer went into a position too small for it, or into one whose signedness can\'t hold it. Widening is implicit precisely because it can\'t fail (type.primitives/CV1a); this can, so the site says what to do with a value that doesn\'t fit rather than the compiler guessing. `u64` → `i64` and `i64` → `u64` are both here: the first has values above `i64.MAX`, the second has negatives.",
                    "let small: u8 = big   // error: `i64` doesn\'t fit in `u8`\n// fix: `let small = big.to<u8>()!` (assert it fits), `big.wrap<u8>()` (low bits), or `big.clamp<u8>()` (clamp)"),
                "E0371" => ("arithmetic across signedness", Type,
                    "`+ - * / %` and `& | ^ << >>` need both operands in the same type, and a signed and an unsigned integer have no common one — `u64` can\'t hold a negative `i32`, and `i32` can\'t hold a large `u64`. Widening one side quietly is the conversion C makes, and it\'s why `-1 < 1u` is true there; Rask asks instead. Comparison is the deliberate exception (type.operators/ORD4): `5u64 > -1i32` answers by value, so there is nothing to guess. Convert one side and say what happens when it doesn\'t fit.",
                    "let u: u64 = 5\nlet i: i32 = -10\nlet sum = u + i   // error: `+` between `u64` and `i32`\n// fix: `let sum = u + i.wrap<u64>()`, or work in i64: `u.wrap<i64>() + i`\nlet ordered = u > i   // fine — comparison crosses signedness (ORD4)"),
                "E0372" => ("write through a binding", Type,
                    "A name a test or a pattern introduced isn\'t a slot. `if x? as v`, `r is E as e`, a match-arm payload and `catch e =>` all name the value the test proved was there, read out of the scrutinee (type.optionals/OPT19 binds a let) — a write to the name would land on that copy and never reach the original. A `for` element is the same, with a different remedy: a plain `for` walks read-only, and `for mutate x in xs` is the mode whose writes reach the collection (std.iteration/I1, I4). This used to be E0322, whose fix was \"add `mut`\" — not writable at any of these sites, since `if x? as mut v` doesn\'t parse.",
                    "if opt? as t { t.n += 1 }   // error: `t` is a binding\n// fix: copy out, then write the whole value back\nif opt? as t {\n    mut next = Counter { n: t.n + 1 }\n    opt = next\n}\n\nfor c in xs { c.n += 1 }   // error: `c` is a read-only element\n// fix: `for mutate c in xs { c.n += 1 }`"),
                "E0373" => ("missing call-site `mutate` marker", Type,
                    "An argument going into a `mutate` parameter is written `mutate arg` (mem.parameters/PM4). This is asymmetry, not ceremony: a misread *move* is already caught — using a value after it moved is a compile error — but nothing catches a misread mutation, because `apply(player, 10)` reading as \"looks at player\" and \"rewrites player\" are both legal code. So the reading the compiler can\'t backstop is the one the source states. PM5: the marker follows the signature, never the argument\'s size, so a Copy argument writes it too. A method receiver is exempt — `player.take_damage(10)` operates on the receiver by construction. The marker on a parameter that *isn\'t* `mutate` is E0328: a marker with no mutation behind it is a lie.",
                    "func apply_damage(mutate p: Player, amount: i64) { … }\napply_damage(player, 10)   // error: mark it at the call site\n// fix: `apply_damage(mutate player, 10)`\n\nplayer.take_damage(10)     // fine — receivers are exempt"),
                "E0365" => ("`take` on a non-optional place", Type,
                    "`take slot` moves the payload out and leaves `none` behind (type.optionals/OPT32), so the place has to have an absent branch to leave. A place that is always there has none: reading it is just `slot`. On a `T or E` the answer is `match` — an error is not a slot you empty. The place\'s type often comes from a struct field, so this is reported once that field resolves rather than at the `take`.",
                    "struct Connection { pending: Request }\nlet req = take conn.pending   // error: `Request` is not a `T?` place\n// fix: declare it `pending: Request?`"),
                "E0831" => ("`??` on a value that is always there", Type,
                    "`??` supplies the branch a `T?` has and a plain value doesn't (type.optionals/OPT3, OPT11). On a left side that can never be absent there is nothing for the right side to be, so the operator has no meaning and no lowering. The usual way to get here is indexing: `m[k]` and `v[i]` panic when the element isn't there rather than handing back a `T?`, so a `??` after one reads like a miss-handler and isn't. Ask for the optional directly with `.get(k)`. For a `T or E` the answer is different — that's a failure, not an absence, and it's `catch` (E0364).",
                    "let v: i64 = m[key] ?? -1   // error: `m[key]` is an `i64`, always there\n// fix: `let v: i64 = m.get(key) ?? -1`"),
                "E0830" => ("`!` on an optional", Type,
                    "`!` negates a `bool`; a `T?` doesn't coerce to `T` (type.optionals/OPT5), so `!` doesn't reach through the wrapper. This matters most on `bool?`, where the payload's type makes `!x` look applicable — but a reader can't tell whether it negates the payload or tests for absence, and `x!` already means force-unwrap on the same operand. Test presence with `x is none`, or narrow first with `if x? as v { !v }`.",
                    "let x: bool? = flag()\nlet y = !x   // error: negation doesn't reach through the optional\n// fix: `if x? as v { !v }`, or `x is none` to test absence"),
                "E0844" => ("`using` context on the entry point", Type,
                    "A `using` clause is a hidden parameter (mem.context/CC11): callers pass the context in, and the compiler finds it by searching the caller's scope. The entry point has no caller — the process starts there — so that parameter is never written and holds whatever the stack happened to contain. Own the context instead: build it as a local in the entry point and call the functions that declare `using`; they resolve it out of your scope automatically.",
                    "func main() using players: Pool<Player> {   // error: nothing can supply this\n    spawn_wave(10)\n}\n\n// fix: own the pool in main, leave the clause on the callee\nfunc main() {\n    mut players: Pool<Player> = Pool.new()\n    spawn_wave(10)   // resolves `players` from main's scope\n}\n\nfunc spawn_wave(n: i64) using players: Pool<Player> { }"),
                "E0841" => ("`@tag` on a variant with an unnamed payload", Type,
                    "Internal tagging writes the tag as a field *inside* the payload\'s object, beside the payload\'s own fields (std.encoding/E24). That needs field names to write: a variant whose payload is unnamed has exactly one value and no key to put it under, so the tag and the payload can\'t share the object without the compiler inventing a name. Either name the field, or drop `@tag` and let the variant encode externally (E22/E23), where the variant name is the key and an unnamed payload goes in directly.",
                    "@tag(\"type\")\nenum Shape {\n    Circle(f64)          // error: unnamed payload can\'t carry the tag\n}\n// fix: name it\n@tag(\"type\")\nenum Shape {\n    Circle { radius: f64 }   // → {\"type\": \"Circle\", \"radius\": 1.0}\n}"),
                "E0842" => ("`@tag` name collides with a payload field", Type,
                    "The tag is written as a field in the same object as the payload\'s own fields (std.encoding/E24), so a payload field with the tag\'s name would produce the key twice. Duplicate keys aren\'t valid JSON, and a decoder reading it back keeps only one — losing either the discriminant or the field. Nothing here is guessable, so it\'s rejected at the declaration: rename the tag or rename the field.",
                    "@tag(\"kind\")\nenum Event {\n    Click { kind: string }   // error: `kind` is both the tag and a field\n}\n// fix: rename one of them\n@tag(\"kind\")\nenum Event {\n    Click { button: string }\n}"),
                "E0843" => ("growing a fixed-size array", Type,
                    "A fixed array's length is part of its type: `[i32; 3]` is a different type from `[i32; 4]`, and the storage is exactly three elements wide with nothing after it to grow into. So `push`, `pop`, `insert` and the rest of the length-changing surface aren't operations it has. Until this was rejected they type-checked through the `Vec` method table and then did real damage: the interpreter quietly grew the value past its own type, and native read the first elements as a Vec header and walked off into whatever they happened to spell. Annotate the slot as `Vec<T>` if it needs to grow — the same literal builds one.",
                    "mut a: [i32; 3] = [1, 2, 3]\na.push(4)                    // error: `[i32; 3]` always holds 3 elements\n\n// fix: say it can grow\nmut a: Vec<i32> = [1, 2, 3]\na.push(4)                    // 4 elements"),
                "W0907" => ("multi-field update under a lock without staged()", Type,
                    "Two or more fields of a locked value written in one `with` block, without staging the update. Rask has no lock poisoning: a panic between the two writes releases the lock and the next task in reads whatever landed, so a multi-field invariant other tasks depend on can be observed half-done (ctrl.panic/LK3). `staged()` works on a copy and commits it as one move, which makes that impossible by construction — the clone is the price, and the method name is where it shows. This is a warning rather than an error because partial state is often harmless; where it is, `@allow(torn_lock_update)` on the enclosing function says so. Mutating method calls don't count: a method body is opaque, and flagging every pair of calls would drown the real signal.",
                    "with accounts.write() as a {\n    a.checking = a.checking - amount   // warning: first field\n    a.savings = a.savings + amount     // second\n}\n\n// fix: one commit, or nothing\nwith accounts.staged() as a {\n    a.checking = a.checking - amount\n    a.savings = a.savings + amount\n}"),
                "E0846" => ("`staged()` outside a `with` block", Type,
                    "Staged access takes the lock, binds a working copy, and commits it back as one move when the block exits — that exit is where the commit happens, so there has to be a block. `read` and `write` also have an expression-scoped form, `box.write().field`, where the lock is held for just that chain; staged has no equivalent, because a single expression has no exit to commit at. Until this was rejected the interpreter reported an internal \"no method `staged`\" and native failed codegen with \"Function not found\".",
                    "let n = C.staged().checking        // error: no block to commit at\n\n// fix: a block, or plain exclusive access for one field\nwith C.staged() as a { a.checking = a.checking - 10 }\nlet n = C.write().checking"),
                "E0845" => ("`staged()` under the `Local` strategy", Type,
                    "Staged access takes the exclusive lock, works on a copy, and commits it as one move — so a panic mid-update leaves the last committed state rather than a half-written one. That matters only when another task could read the torn state. `Shared<T, Local>` is the single-task strategy: it takes no lock at all, nothing else can see the value, and a panic that unwinds past the block kills the only task that could. So the clone protects nothing and costs a copy, which is why it is refused at the call site rather than quietly performed.",
                    "const C: Shared<Acc, Local> = Shared.local(Acc { n: 1 })\nwith C.staged() as a { … }      // error: nothing else can observe a tear\n\n// fix: plain exclusive access\nwith C.write() as a { … }"),
                "E0847" => ("`try` inside `ensure`", Type,
                    "An `ensure` body runs at scope exit, on the way out of the function — by then the return value is already decided, and on a panic there is no return at all. So there is nothing for `try` to propagate an error to. That is why cleanup errors are ignored by default (ctrl.ensure/ER1) and `else |e|` exists to observe them (ER2); the same reasoning bars `try` from the handler, which is the last thing that runs (ER3). Until this was rejected the program type-checked, did nothing visible on the interpreter, and failed native codegen with an internal message about a type it couldn\'t work out.",
                    "ensure { let n = try f.close() }        // error: nowhere to propagate to\n\n// fix: observe the error here instead\nensure f.close() else |e| { log(e.message()) }"),
                "E0848" => ("comptime test failed", Type,
                    "A `comptime test` runs during compilation, not in the test runner (std.testing/T11), so anything that stops it — a false `assert`, or a body the compiler can't evaluate — is a compile error. The second half is the common one: the compiler has no files, no sockets and no scheduler, only the comptime subset (ctrl.comptime/CT7), so a test that opens a file or reaches `spawn` can't run there at all. Drop the `comptime` and it becomes an ordinary test, reported by the runner like any other.",
                    "comptime func fact(n: i64) -> i64 {\n    if n <= 1 { return 1 }\n    return n * fact(n - 1)\n}\n\ncomptime test \"factorial\" {\n    assert fact(5) == 121      // error: left: 120, right: 121\n}\n\n// fix: correct the expectation — or, if the body needs a real runtime,\n// drop `comptime` and let the runner have it\ntest \"reads the fixture\" {\n    let text = try fs.read(\"fixture.json\")\n}"),
                "E0849" => ("ambiguous pool context", Type,
                    "A call needs a `using Pool<T>` context and the caller has more than one `Pool<T>` in scope at the same priority, so there is no rule that picks one (mem.context/CC8). Contexts resolve by searching the caller's scope, which only works while the answer is unique; guessing here would silently send the handle to the wrong pool. Name the pool at the call — pass it as an ordinary parameter, or index it directly — and the ambiguity is gone.",
                    "mut alive = Pool<Player>.new()\nmut dead = Pool<Player>.new()\nlet h = alive.insert(p)\ndamage(h, 10)              // error: which Pool<Player>?\n\n// fix: say which\ndamage(alive, h, 10)       // pool as an ordinary parameter\nalive[h].health -= 10      // or index it here"),
                "E0850" => ("storable closure can't inherit a context", Type,
                    "A closure bound to a name can be stored and called later, after the scope that owns the pool is gone — so it cannot capture an ambient `using Pool<T>` the way an inline callback can (mem.context/CC10). An inline callback runs inside the scope that resolved the context, which is what makes that case safe. Take the pool as an explicit closure parameter and pass it at each call.",
                    "let cb = |h| { pool[h].health -= 10 }   // error: `cb` outlives `pool`\n\n// fix: take the pool as a parameter\nlet cb = |pool: Pool<Player>, h| { pool[h].health -= 10 }\ncb(alive, h)"),
                "E0851" => ("stale handle access", Type,
                    "Typestate analysis followed this handle through the control flow and proved it was removed before this access (comp.advanced/TS8). A handle is not a pointer — the pool checks a generation on every access — so this would panic at run time rather than read freed memory; the analysis is what turns the panic into a compile error where it can prove it. Where it cannot prove it, the runtime check still holds.",
                    "let h = pool.insert(Player { health: 100 })\npool.remove(h)\npool[h].health -= 10        // error: `h` was removed above\n\n// fix: ask whether it is still there\nif pool.get(h) is Some {\n    pool[h].health -= 10\n}"),
                "E0840" => ("resource discarded as a statement", Ownership,
                    "A resource-typed value (marked @resource, like `TaskHandle` — conc.async/H1) came back from a call used as a bare statement, with nothing to bind it to. The value is produced and dropped in the same instant, before anything could consume it — the same leak `E0805` catches for a named binding that falls out of scope unconsumed, just with no name to point at.",
                    "using Multitasking {\n    spawn(|| { work() })   // error: TaskHandle dropped without join()/detach()\n}\n// fix: let h = spawn(|| { work() })\n//      h.detach()"),
                // ─── Backfill: emitted by convert.rs, previously unexplained (#892) ───
                "E0211" => ("C header could not be parsed", Resolution,
                    "`import c` runs the built-in C parser over the header, and it handles standard C — not C++, and not compiler-specific extensions. A header that pulls in templates, `__attribute__` forms the parser doesn't know, or vendor builtins stops here. The way past it is to skip the header for those declarations and write the handful you need by hand: an `extern \"C\"` block declares the symbol and its signature directly, and nothing has to be parsed.",
                    "import c \"vendor/simd_intrin.h\"   // error: can't parse this header\n\n// fix: declare just what you call\nextern \"C\" {\n    func vendor_add(a: i32, b: i32) -> i32\n}"),
                "E0212" => ("module has no such symbol to import", Resolution,
                    "A selective import — `import time.Duration` — names one symbol out of one module, and the module doesn't export that name. Usually a typo or a name that moved. This used to be accepted at resolution and only failed much later, at code generation, with an error that pointed at generated code instead of at the import line.",
                    "import string.Builder      // error: `string` has no `Builder`\n// fix: import string.StringBuilder\n//      or import the whole module: import string"),
                "E0213" => ("`extern` name already declared with a different signature", Resolution,
                    "An `extern` name is one symbol in the linked program, so two signatures for it can't both be right. The stdlib declares some C functions itself — std.fs declares `strlen` — and a second declaration used to replace it silently, which left the stdlib's own calls type-checked against your signature instead of theirs. Match the existing declaration, or drop yours: the name is already in scope.",
                    "extern \"C\" {\n    func strlen(s: *u8) -> i32   // error: already declared as -> usize\n}\n// fix: drop it — `strlen` is already in scope from std.fs"),
                "E0332" => ("Self-returning method called through a trait object", Trait,
                    "`as any Trait` erases the concrete type, and a method returning `Self` has to name it — the caller would have no type to put the result in (type.traits/TR2). This is a property of the method, not of the call: the same method is fine on the concrete type, where `Self` is known. Split the trait if only some methods need erasing, or call this one before boxing.",
                    "trait Shape { func scaled(self, k: f64) -> Self }\nlet s: any Shape = circle\nlet bigger = s.scaled(2.0)   // error: what type is the result?\n// fix: scale first, then erase\nlet bigger: any Shape = circle.scaled(2.0)"),
                "E0334" => ("public function is missing type annotations", Type,
                    "A public function's signature is its contract, so it's written out rather than inferred (struct.grouping/GC5). Inference still works inside the body, and on anything not `public` — the rule is about what callers can see. A caller reading the declaration should not have to read the body to learn what goes in and what comes out.",
                    "public func scale(v, k) {        // error: no types on `v`, `k`, or the return\n    return v * k\n}\n// fix: write the contract\npublic func scale(v: f64, k: f64) -> f64 {\n    return v * k\n}"),
                "E0336" => ("value used after `discard`", Ownership,
                    "`discard name` drops a value and invalidates the binding on purpose — it's the way to say \"this is finished with\" for a type that would otherwise be flagged as unconsumed. After it, the name holds nothing. Either drop the `discard` and go on using the value, or move the `discard` past the last use.",
                    "let buf = load()\ndiscard buf\nprintln(\"{buf.len}\")   // error: `buf` was discarded above\n// fix: move the discard after the last use"),
                "E0337" => ("range step of zero", Type,
                    "A zero step never advances, so the loop would run forever without making progress (ctrl.ranges/SP3). A step's sign also picks the direction: positive counts up, negative counts down. Zero says neither.",
                    "for i in (0..10).step(0) { … }   // error: never advances\n// fix: `.step(2)` to count up by two, `.step(-1)` on a descending range"),
                "E0339" => ("`.read()`/`.write()` with nothing chained onto it", Type,
                    "Taking a lock on a `Shared` is expression-scoped: the lock is held for the chain it appears in and released at the end of it (mem.borrowing/E5). On its own, with no field access after it, the lock is taken and dropped in the same instant and the value goes nowhere. Chain a field to read one thing, or use `with` when you need several statements under the lock — that form holds it for the block.",
                    "counter.write()             // error: locks, then immediately unlocks\n// fix (one field): counter.write().hits = 0\n// fix (several statements):\nwith counter.write() as c {\n    c.hits = 0\n    c.last = now()\n}"),
                "E0347" => ("type pattern names a type the result can't hold", Type,
                    "A type pattern matches one arm of a result's error union, so the type it names has to be in that union (type.errors/ER23). Naming anything else is a branch that can never be taken — most often a stale error type left after the signature changed, or a typo. The union is written in the function's return type; match against what's there.",
                    "func load() -> Config or (ParseError | IoError) { … }\nmatch load() {\n    NetworkError as e => …    // error: not in `ParseError | IoError`\n}\n// fix: match one of the two it can actually return"),
                "E0348" => ("`Some(...)`/`Ok(...)`/`Err(...)` used as a constructor", Type,
                    "Rask has no wrapper constructors. A `T?` is built by writing the value or `none`, and a `T or E` by returning either side — the wrapping happens at the `return`, from the signature (type.optionals/OPT2, type.errors/ER2). There is nothing to call, which is the point: no name to import, no distinction between a `T` and a wrapped `T` at the call site.",
                    "return Some(user)     // error: `Some` isn't callable\nreturn Ok(user)       // error: `Ok` isn't callable\n// fix: just return the value — the signature says which branch it is\nreturn user"),
                "E0349" => ("`match` on an optional", Type,
                    "An optional has exactly two states, and the operator family covers both in less space than a match with two arms (type.optionals). `if x?` tests presence, `if x? as v` binds the payload, `x ?? d` supplies a fallback, and `x == none` tests absence. A `match` here would also need pattern names that don't exist.",
                    "match maybe_user { … }        // error: not a user enum\n// fix, depending on what you want:\nif maybe_user? as u { … }     // bind the payload\nlet name = maybe_user?.name ?? \"anon\"\nif maybe_user == none { return }"),
                "E0350" => ("`Some`/`None`/`Ok`/`Err` used as a pattern", Type,
                    "The same rule as E0348, on the pattern side: optionals and results have no variant names to match, so there is nothing for `Some(v)` to destructure (type.optionals/OPT2, type.errors/ER2). Presence and absence are operators; a result's error arms are matched by their actual error types.",
                    "if r is Ok(v) { … }              // error: `Ok` isn't a pattern\n// fix: operators for presence\nif r? as v { … }\n// fix: the real error type for the failure arm\nif r is ParseError as e { … }"),
                "E0362" => ("`try` on a value that can both fail and be absent", Type,
                    "A `T? or E` has two ways to leave the function and `try` only handles one, so which one it means would be a guess (type.errors/ER47, ER16b). Say both: `try` sends the error up, and `??` answers the absence here. The order reads left to right — propagate the failure, then deal with the miss.",
                    "let cfg = try load()          // error: error, or absence?\n// fix: one answer for each\nlet cfg = try load() ?? default_config()\nlet cfg = try load() ?? return none"),
                "E0363" => ("`catch` on an optional", Type,
                    "`catch` binds or drops an error, and an absence isn't one — `none` carries nothing to bind (type.errors/ER14). The fallbacks are split by shape on purpose: `??` for a miss, `catch` for a failure. Reaching for `catch` here usually means the value is more optional than you thought.",
                    "let port = read_port() catch _ => 8080   // error: nothing to catch\n// fix: `??` is the optional's fallback\nlet port = read_port() ?? 8080"),
                "E0366" => ("`take` on a `let` binding", Type,
                    "`take slot` moves the payload out and writes `none` back (type.optionals/OPT32) — that second half is a mutation, so the place has to be writable. A `let` isn't. If you only want to read the value and leave it there, drop the `take`.",
                    "let pending: Request? = …\nlet req = take pending    // error: `pending` isn't writable\n// fix: mut pending: Request? = …\n// or, if it should stay: let req = pending"),
                "E0367" => ("method call on a `T?` or `T or E`", Type,
                    "The wrapper shapes are operator-only: one spelling per job, and the right-hand side stays lazy by construction (std.stdlib/api-design SD4). So there is no `unwrap`, `expect`, `ok_or` or `and_then` to call — `x!` forces, `??` supplies a fallback, `x?.field` reaches through, `catch` handles a failure. Reaching for a method here is usually Rust muscle memory; the operator for the same job is shorter.",
                    "let n = maybe_count.unwrap_or(0)    // error: no methods on `i64?`\n// fix: the operator that does that job\nlet n = maybe_count ?? 0"),
                "E0368" => ("`?` on a result", Type,
                    "`?` asks whether a value is there, and a result answers a different question: it succeeded or it failed, and the failure carries an error (type.errors/ER12). Treating it as presence would step over that error without naming it. Test the failure with `is`, or handle it with `catch`.",
                    "if load()? { … }                     // error: this is a result\n// fix: name the failure\nif load() is ParseError as e { … }\nlet cfg = load() catch e => fallback(e)"),
                "E0382" => ("comparing two things that aren't the same type", Type,
                    "The two sides of a comparison have to be the same type, with one deliberate exception: two integers compare across signedness (type.operators/ORD4). `char` is not in that exception, because a `char` is a Unicode scalar rather than a number — comparing it to an integer answers by code point, which is right for ASCII and silently wrong for everything else. It bites hardest next to byte indexing: `s[i]` is a `u8` (std.strings/U1b), so `line[i] == ','` reads like a character test and means `line[i] == 44`.",
                    "if line[i] == ',' { … }        // error: `u8` against `char`\n// fix: say which one you meant\nif line[i] == 44u8 { … }                   // the byte\nif line.char_at(i)? as c { c == ',' }      // the character"),
                "E0379" => ("`Link` outlives the rack it points into", Ownership,
                    "A `Link<T>` is the address of a node, and the nodes live in the rack — so when the rack goes out of scope the node goes with it and the link dangles. Nothing else catches this: no `delete` happened, so the use-after-delete rule never looks, and a link is Copy, so it escapes the scope that produced it without a move to flag. A link into a rack the *caller* owns is fine, because that rack outlives the call.",
                    "func build() -> Link<Node> {\n    mut r: Rack<Node> = Rack.new()\n    return r.add(Node { v: 1 })   // error: `r` dies at the return\n}\n// fix: let the caller own the rack\nfunc build(mutate r: Rack<Node>) -> Link<Node> {\n    return r.add(Node { v: 1 })\n}"),
                "E0808" => ("collection restructured inside its own `with` block", Ownership,
                    "A `with` block borrows one element in place, and `push`, `insert`, `remove` or a resize can move the whole buffer — which would leave the binding pointing at freed memory. Do the structural change outside the block. A `Pool` is the way to hold a reference across one: handles survive reallocation, because they're an index and a generation rather than an address.",
                    "with items[0] as first {\n    items.push(other)      // error: may reallocate under `first`\n}\n// fix: move it out\nwith items[0] as first { use(first) }\nitems.push(other)"),
                "E0809" => ("removing the very element a `with` block is holding", Ownership,
                    "The binding is a view into that element's storage, and removing it frees the storage — the view would dangle for the rest of the block. This is the narrow case of E0808 worth its own message, because the fix is different: it isn't \"move the mutation out of the way\", it's \"finish with the element, then remove it\".",
                    "with pool[h] as node {\n    pool.remove(h)         // error: that's the element you're holding\n}\n// fix: leave the block first\nwith pool[h] as node { use(node) }\npool.remove(h)"),
                "E0814" => ("collection restructured during `for mutate`", Ownership,
                    "`for mutate` walks elements in place so writes reach the collection, which means the loop holds a position in it. Insert, remove, push or clear shifts the elements or moves the buffer, and the position no longer means what it did — the loop would skip elements, repeat them, or read freed memory. Writing to the elements is fine; changing how many there are is not. Collect the changes and apply them after.",
                    "for mutate item in items {\n    if item.dead { items.remove(i) }   // error: invalidates the walk\n}\n// fix: decide during, apply after\nmut doomed: Vec<i64> = []\nfor item in items { if item.dead { doomed.push(item.id) } }\nfor id in doomed { items.remove_by_id(id) }"),
                "E0815" => ("`for mutate` element passed to a `take` parameter", Ownership,
                    "`for mutate` lends each element in place — the collection still owns it. A `take` parameter consumes what it's given, which would leave a hole in the collection with nothing written back. Clone the element if the callee really needs its own, or change the callee to `mutate`, which writes through instead of consuming.",
                    "for mutate c in conns {\n    close(take c)          // error: `conns` still owns `c`\n}\n// fix: let the callee borrow it\nfor mutate c in conns { reset(mutate c) }"),
                "E0816" => ("`_` would drop a linear value", Ownership,
                    "A linear value is consumed exactly once (mem.linear/L1), and `_` consumes nothing — it discards the binding, so a resource or an `Owned<T>` matched into it is never closed or freed. Name it instead and consume it on every arm. This applies to the scrutinee and to payload fields alike: a `_` in a field position drops that field just as silently.",
                    "match conn {\n    _ => return              // error: the connection is never closed\n}\n// fix: name it and consume it\nmatch conn {\n    c => { c.close(); return }\n}"),
                "E0828" => ("value doesn't auto-wrap outside a `return`", Type,
                    "A plain value becomes a `T or E` at a `return`, where the signature says which branch it is. At an assignment there is no signature to read, so the choice between the success and the error side would be a guess — and it's written instead (type.errors/ER11). Optionals are exempt: a `T` widens to a `T?` anywhere, because `none` is the only other branch. Get the result from something that already returns one — a call, or a small `func` whose `return` does the wrapping.",
                    "let r: Config or ParseError = cfg    // error: which branch is `cfg`?\n// fix: let a return do the wrapping\nfunc ok(c: Config) -> Config or ParseError { return c }\nlet r = ok(cfg)"),
                "E0839" => ("`with shared as g` doesn't say which lock", Type,
                    "A `Shared` is read by many or written by one, and the two behave differently — a read binding lets other readers in and never writes back, a write binding shuts them out and does (conc.sync/R4). Which one you get is written rather than inferred, because the difference is not visible in the block's body but is very visible in production.",
                    "with counter as c { … }          // error: read or write?\n// fix: name the lock\nwith counter.read() as c { … }   // concurrent readers\nwith counter.write() as c { … }  // exclusive"),
                "E0383" => ("comptime evaluation failed", Type,
                    "`const X = comptime { … }` says the value is computed while the program is being compiled (ctrl.comptime/CT2), so there is no second chance: if the block panics, runs past the branch quota, indexes off the end, or asks for something that only exists at run time (I/O, a pool, a spawn), the constant has no value and compilation stops. That is the deal the keyword makes — the alternative, quietly running the block at startup instead, turns a compile error into a crash in the field.\n\nA long-running fold that is genuinely finite is the one case to override: `@comptime_quota(N)` on the const raises the backwards-branch limit from its 1,000 default (CT35).",
                    "const PRIMES = comptime { sieve(100000) }   // error: quota (1,000)\n// fix: say how much room it needs\n@comptime_quota(500000)\nconst PRIMES = comptime { sieve(100000) }"),
                "E0384" => ("atomic payload doesn't fit one word", Type,
                    "An atomic is a value the hardware reads and writes in a single instruction, which means one machine word. `Atomic<T>` takes any payload that fits — every integer width, `bool`, a float, or a struct whose data is one word. Anything wider has no single instruction behind it, so there is nothing to make atomic (mem.atomics/GA2).\n\nRask gives every struct field its own word, so a two-field struct is 16 bytes however small the fields are written. `Shared<T, Mutex>` is the answer for a payload that size — it costs a lock, which is the honest price.",
                    "struct Slot { index: i32, gen: i32 }   // two fields, 16 bytes\nlet s = Atomic<Slot>.new(…)          // error: doesn't fit one word\n// fix: one word of data\nstruct Slot { packed: i64 }\nlet s = Atomic<Slot>.new(Slot { packed: 0 })"),
                "E0385" => ("the field name in `value.(…)` isn\'t known at compile time", Type,
                    "`value.(expr)` is not dynamic field access — it is a compile-time rewrite to a direct field access, which is why it costs nothing at run time (ctrl.comptime/CT53). The name therefore has to be one the compiler can read: a string literal, a `comptime { … }` block, a `let` bound to either, or a `comptime for` binding\'s `.name`. A string that only exists once the program is running has nothing to rewrite to.\n\nA `mut` binding never qualifies, however it was initialised — it can be reassigned, so the name it holds at the access isn\'t decidable here.",
                    "let which = pick(n)          // a runtime string\nprintln(\"{b.(which)}\")       // error: not known at compile time\n// fix: name it, or fold it\nlet which = comptime { \"limit\" }\nprintln(\"{b.(which)}\")"),
                "E0214" => ("C header not found", Resolution,
                    "`import c \"header.h\"` reads a real file: the compiler parses the header to learn the declarations it is being asked to trust, so a header it can't open is a hard stop rather than a name it can guess at. The path is searched the same way a C compiler searches it — system include directories plus the project's own — so a missing one usually means the library's development package isn't installed, or the include path doesn't reach it.",
                    "import c \"sqlite3.h\"          // error: C header not found\n// fix: install the dev package, or point at the header\n// apt install libsqlite3-dev"),
                "E0215" => ("`break` names neither a value nor a label", Resolution,
                    "`break` does two jobs and the name after it says which: `break x` leaves the loop carrying `x`, and `break 'outer` jumps out of the loop wearing that label. A name that is neither a variable in scope nor a label on an enclosing loop can't be either, and guessing between them would silently turn a value into a jump.\n\nThe message lists the labels the enclosing loops do carry, which is usually enough to spot a typo.",
                    "loop {\n    break total          // error: `total` is neither\n}\n// fix: bind it first, or label the loop\nmut total = 0\nloop { break total }"),
                "E0300" => ("type expression isn't a type", Type,
                    "A type annotation has to name something the compiler can resolve: a primitive, a declared struct or enum, or one of those with generic arguments. This text isn't any of them — usually a typo, a Rust spelling (`Vec<u8>` is right, `&[u8]` isn't), or a value used where a type belongs.",
                    "let xs: vec<i64> = []        // error: invalid type `vec<i64>`\n// fix: types are PascalCase\nlet xs: Vec<i64> = []"),
                "E0301" => ("the type parameter's bounds don't declare this method", Type,
                    "Inside a generic function the only thing known about `T` is what its bounds say, so a call has to be one of the methods a bound declares. This one isn't — which means either the bound is missing or the method belongs on a different type.\n\nThis is the deliberate half of Rask's generics: a body is checked once, against the bounds, rather than re-checked per instantiation. The cost is that a method has to be promised before it can be called.",
                    "func largest<T>(xs: Vec<T>) -> T {\n    return xs.max()          // error: no `max` in T's bounds\n}\n// fix: promise it\nfunc largest<T: Comparable>(xs: Vec<T>) -> T { return xs.max() }"),
                "E0302" => ("cannot mutate a `let` binding", Type,
                    "`let` and `mut` are the whole of Rask's mutability story: a `let` name can't be reassigned and can't have a mutating method called on it. That's not ceremony — it is what makes a reader able to tell, from the declaration alone, whether a name's value can change under them.",
                    "let count = 0\ncount = count + 1            // error: `count` is a let binding\n// fix: say it changes\nmut count = 0\ncount = count + 1"),
                "E0303" => ("a string view can't outlive the statement that made it", Type,
                    "Slicing a string gives a view: sixteen bytes pointing into the source's buffer, with no copy. That's the point — it costs nothing — and it's also why it can't be stored. The moment the source is reassigned or goes out of scope, a stored view points at bytes that no longer exist.\n\nUse the view where it is made, or call `.to_string()` to take a copy that owns its own buffer and can be kept.",
                    "let head = text[0..4]        // error: this view can't outlive the line\n// fix: copy it out\nlet head = text[0..4].to_string()"),
                "E0304" => ("a guard's `else` block has to leave", Type,
                    "`if x? as v else { … }` binds `v` for everything after the `if`, not just inside it. That is only sound when the `else` path never reaches the code that uses the binding, so the block has to end in `return`, `break`, `continue`, or a panic. A block that falls through would leave `v` naming nothing.",
                    "if parse(s)? as n else { log(\"bad\") }   // error: `else` falls through\nprintln(\"{n}\")\n// fix: leave\nif parse(s)? as n else { return }\nprintln(\"{n}\")"),
                "E0305" => ("an argument being given away is marked `own` at the call", Type,
                    "A parameter declared `own` takes the value: the caller can't use it afterwards. That's visible in the signature but not at the call site, so Rask makes the call site say it too. The same reasoning as `mutate` (mem.parameters/PM4): a misread move is caught by the compiler later, but the reader shouldn't have to look up the signature to see that a value is being handed over.",
                    "consume(buffer)              // error: `buffer` needs `own`\n// fix: say it\nconsume(own buffer)"),
                "E0306" => ("a parameter marked with something it doesn't declare", Type,
                    "`mutate`, `own` and `deleting` at a call site each match a parameter that declares them. Writing one the signature doesn't ask for is a lie in the other direction — it reads as though the callee does something it doesn't — so it's rejected rather than ignored.",
                    "func log(msg: string) { … }\nlog(mutate msg)              // error: `msg` isn't a `mutate` parameter\n// fix: drop the marker\nlog(msg)"),
                "E0329" => ("a function that deletes nodes has to declare `deleting`", Ownership,
                    "Deleting from a rack revokes every link into it, including links the caller is holding and never passed in. A signature that doesn't say so leaves the caller with names that quietly stop being valid, which is precisely the thing links are supposed to make impossible.\n\n`deleting r: Rack<…>` is the declaration. The alternative, when the function only ever deletes what it was handed, is to take those links as `take` parameters instead — then nothing outside the call is affected.",
                    "func prune(r: Rack<Node>, n: Link<Node>) {\n    r.delete(n)              // error: this can delete nodes the caller never named\n}\n// fix: declare it\nfunc prune(deleting r: Rack<Node>, n: Link<Node>) { r.delete(n) }"),
                "E0330" => ("a `deleting` parameter is marked `deleting`, not `mutate`", Type,
                    "A `deleting` parameter is a `mutate` parameter that may also delete nodes the caller never named, so your links into that rack are revoked at this call. Those are different contracts, and printing them the same at the call site would hide the more serious one.\n\nThe fix is one token, and it's worth seeing here rather than discovering at the next read (mem.parameters/PM4, PM5).",
                    "prune(mutate scene, doomed)  // error: `prune` can delete from `scene`\n// fix: say which contract\nprune(deleting scene, doomed)"),
                "E0388" => ("this type can't be encoded or decoded", Type,
                    "`Encode`/`Decode` aren't written by hand — a type has them when its fields do, all the way down (std.encoding/E12). So this error names the field that stops it: something with no wire representation, like a file handle, a channel or a function.\n\nEither give the field a serializable type, or mark it `@no_serialize` to leave it out of the format and fill it in after decoding.",
                    "struct Session { id: i64, conn: TcpStream }\n// error: `Session` cannot be encoded — `conn`\n// fix: leave it out\nstruct Session { id: i64, @no_serialize conn: TcpStream }"),
                "E0335" => ("`+` doesn't join strings", Type,
                    "Joining strings allocates, and Rask keeps allocation visible at the call. `+` reads as free, so it isn't the spelling: interpolation shows the whole result being built in one place, and `StringBuilder` shows one allocation reused across many appends.\n\nThere is no `concat` either — one spelling per operation (std.api/SD5).",
                    "let full = first + \" \" + last      // error: `+` on strings\n// fix: write the pieces\nlet full = \"{first} {last}\""),
                "E0340" => ("`match` doesn't cover every case", Type,
                    "A `match` has to account for every value the scrutinee can be. The message names the variants that are missing — add an arm for each, or a `_` arm for the rest.\n\nExhaustiveness is what makes adding a variant to an enum a compile error at every place that has to change, instead of a silent fall-through at run time.",
                    "match state {\n    Idle => …\n    Running => …            // error: missing `Done`\n}\n// fix: cover it, or say you don't care\nmatch state {\n    Idle => …\n    Running => …\n    _ => …\n}"),
                "E0341" => ("name isn't defined", Type,
                    "Nothing by this name is in scope — check the spelling, or import it. Nothing in Rask comes pre-imported (structure.modules/IM1), so a stdlib name needs the import that brings it in even when it feels built in.",
                    "println(\"{PI}\")              // error: undefined name `PI`\n// fix: bring it in\nimport math\nprintln(\"{math.PI}\")"),
                "E0343" => ("`T or E` needs two different types", Type,
                    "A result's branch is picked by the value's type, so `i64 or i64` has nothing to pick with — a caller could not tell success from failure. The two sides have to differ.\n\nA newtype is the usual fix when both really are the same underlying type: `type ParseError = string` is a distinct type, so `i64 or ParseError` reads apart.",
                    "func find(k: string) -> string or string   // error: both sides are `string`\n// fix: newtype one side\ntype NotFound = string\nfunc find(k: string) -> string or NotFound"),
                "E0344" => ("an error type needs a `message`", Type,
                    "Anything on the error side of a `T or E` has to be able to say what went wrong, which means one method: `func message(self) -> string`. An enum gets it derived from its variants, so this usually means the error is a primitive — and a bare `string` or `i64` carries no meaning to a reader of the failure. Newtype it and give it a message.",
                    "func read(p: string) -> string or i64      // error: `i64` has no `message`\n// fix: give the error a name and words\ntype ReadError = i64\nextend ReadError { func message(self) -> string { return \"read failed: {self.value}\" } }"),
                "E0369" => ("`try` on something that isn't a result", Type,
                    "`try` takes the success side of a `T or E` (or the value of a `T?`) and sends the other branch out to the caller. A value with only one branch has nothing to propagate, so there is nothing for `try` to do.",
                    "let n = try compute()        // error: `compute()` returns `i64`\n// fix: drop the `try`\nlet n = compute()"),
                "E0386" => ("this needs an `unsafe` block", Type,
                    "Raw pointers, C calls and reinterpreting memory are the operations the compiler can't check for you, so they're written inside `unsafe { … }`. The block isn't permission — it's a marker that says \"the invariant here is mine, not the compiler's\", which is what makes it findable later.",
                    "let v = *p                   // error: dereference requires `unsafe`\n// fix: mark the region\nunsafe { let v = *p }"),
                "E0387" => ("`string.new()` doesn't exist", Type,
                    "An empty string is `\"\"`. `string.new()` only ever made sense as the start of a sequence of pushes, and `string` can't be mutated — one spelling per operation (std.api/SD5).\n\nIf that *was* what you wanted, `StringBuilder` is the type that owns its buffer and can be appended to.",
                    "mut s = string.new()         // error: no such constructor\n// fix: an empty string, or a builder\nmut b = StringBuilder.new()"),
                "E0333" => ("type doesn't implement the trait a bound requires", Trait,
                    "A bound is a promise the caller has to keep. This type doesn't keep it — either it's missing the methods the trait declares, or the trait covers a fixed set of types (like the numeric ones) and this isn't one of them.\n\nConformance is nominal (#283): a type has a trait because an `extend T with Trait` block says so, not because its methods happen to line up.",
                    "func total<T: Numeric>(xs: Vec<T>) -> T { … }\ntotal(names)                 // error: `string` does not implement `Numeric`\n// fix: pass numbers, or widen the bound"),
                "E0389" => ("a resource can't be discarded", Type,
                    "`discard` throws a value away. A `@resource` has to be consumed exactly once by something that closes it, and throwing it away is the leak the linearity rules exist to prevent — a file that's never closed, a transaction that's never committed or rolled back.",
                    "discard file                 // error: `File` is a resource\n// fix: consume it properly\nfile.close()"),
                "E0390" => ("a public function has to name its error types", Type,
                    "A `_` error type is inferred from the body, which is fine inside a package but not across its edge: the signature is the contract, and a caller can't see a union that only exists after the body is checked. Write the errors out (ER21).",
                    "public func load(p: string) -> Config or _    // error: `_` in a public signature\n// fix: name them\npublic func load(p: string) -> Config or (IoError or ParseError)"),
                "E0391" => ("an enum mixes explicit and automatic discriminants", Type,
                    "Either every variant gets a `= N` or none does. Mixing them makes the numbering of the unnumbered ones depend on where they sit in the list, which is a silent trap when a variant is inserted (type.enums/E16).",
                    "enum Status { Ok = 200, NotFound, Error = 500 }   // error: mixed\n// fix: number them all\nenum Status { Ok = 200, NotFound = 404, Error = 500 }"),
                "E0392" => ("a nominal type doesn't convert on its own", Type,
                    "`type Meters = f64` makes a distinct type, not an alias — that's the whole point, so a length can't be passed where a duration is wanted. It doesn't convert implicitly in either direction: `Meters(x)` wraps, `.value` unwraps (type.aliases/T9).",
                    "let d: f64 = distance        // error: `Meters` is not `f64`\n// fix: unwrap it\nlet d: f64 = distance.value"),
                "E0393" => ("a variant can't have both a payload and a discriminant", Type,
                    "An enum with explicit discriminants is integer-backed — its values *are* those numbers, which is what lets it cross a wire or an FFI boundary. A variant carrying fields has more than a number in it, so the two can't be combined (type.enums/E17).",
                    "enum Msg { Ping = 1, Data(Vec<u8>) = 2 }   // error: `Data` has both\n// fix: pick one\nenum Msg { Ping = 1, Pong = 2 }"),
                "E0394" => ("two variants share a discriminant", Type,
                    "Explicit discriminants have to be unique — two variants with the same number can't be told apart once the enum is written out and read back (type.enums/E15).",
                    "enum Code { Ok = 0, Done = 0 }   // error: both are 0\n// fix: give them different numbers\nenum Code { Ok = 0, Done = 1 }"),
                "E0395" => ("type aliases form a cycle", Type,
                    "Each alias has to bottom out in a concrete type. A cycle never does, so there is nothing to resolve it to (T6).",
                    "type A = B\ntype B = A                   // error: cyclic\n// fix: break it\ntype A = i64\ntype B = A"),
                "E0396" => ("field is private", Type,
                    "A field with no `public` is reachable only from `extend` blocks on its own type. That's the boundary a struct draws around its invariants — a public method is how the outside asks for the value (V5).",
                    "let n = account.balance      // error: `balance` is private\n// fix: ask for it\nlet n = account.current_balance()"),
                "E0397" => ("`else as e` needs a result to bind", Type,
                    "`else as e` names the error the condition produced, so the condition has to have one — `if r?` on a `T or E`. An optional's absence carries no payload, so there is nothing for `e` to be (type.errors/ER22).",
                    "if find(k)? as v else as e { … }   // error: `find` returns `T?`\n// fix: nothing to bind on an optional\nif find(k)? as v else { … }"),
                "E0398" => ("`is` names something the value can't be", Type,
                    "`is T as name` picks one branch of a two-branch value — a `T or E` or a `T?`. Either the scrutinee has only one branch, in which case the test can never be false, or the type named isn't one of the branches it does have, in which case it can never be true (type.errors/ER23).",
                    "let n: i64 = 3\nif n is string as s { … }    // error: `i64` has no branches\n// fix: test something with two"),
                "E0399" => ("`try` would propagate an absence into a function that returns an error", Type,
                    "Bare `try` sends the operand's other branch out unchanged, so that branch has to fit the return type. Here the operand is a `T?` and the function returns `T or E` — `none` isn't an error, and inventing one would be the compiler choosing what went wrong (type.errors/ER47).",
                    "func load() -> Config or IoError {\n    let raw = try cache[key]     // error: `none` has nowhere to go\n}\n// fix: name the error\nlet raw = cache[key] ?? return IoError.NotFound"),
                "E0400" => ("`try` would propagate an error into a function that returns an optional", Type,
                    "The mirror of E0399. The operand is a `T or E`, the function returns `T?`, and an error doesn't fit an absent branch — the information in it would be thrown away silently (type.errors/ER47).",
                    "func lookup() -> Config? {\n    let raw = try read(path)     // error: the error has nowhere to go\n}\n// fix: drop it where it happens\nlet raw = read(path) catch _ => return none"),
                "E0401" => ("arithmetic between an integer and a float", Type,
                    "An integer and a float in the same operation is a conversion, and a conversion that can lose the value isn't implicit (type.primitives/CV1a). Which loss is acceptable is the program's decision, so it's written at the site: `.round<f64>()` for the usual one, `as f64` only at widths where nothing can be lost.\n\nAn unsuffixed literal is not affected — it takes the other operand's type, so `x + 1` on an `f64` is `x + 1.0`.",
                    "let avg = total / count      // error: `f64` and `i64`\n// fix: say what happens to the integer\nlet avg = total / count.round<f64>()"),
                "E0859" => ("mutation in a frozen context", Ownership,
                    "A `frozen` context clause promises the structure won't change for the duration, which is what lets iteration run without a generation check on every step. A structural mutation inside one would break that promise — remove `frozen`, or move the mutation out.",
                    "func draw(frozen scene: Rack<Node>) {\n    scene.delete(n)          // error: cannot delete in frozen context\n}"),
                "E0860" => ("a `take` parameter was consumed and never replaced", Ownership,
                    "`take x: T` hands the value over for the duration of the call and expects one back — the caller's name still refers to the slot afterwards. Consuming the value and returning without assigning a new one leaves that slot empty.",
                    "func swap(take buf: Buffer) {\n    buf.close()              // error: `buf` is still empty when this returns\n}\n// fix: put one back\nfunc swap(take buf: Buffer) { buf.close(); buf = Buffer.new() }"),
                "E0861" => ("clearing a collection that has a live binding into it", Ownership,
                    "`with xs[i] as e { … }` borrows an element in place. Clearing the collection frees every element, including that one, so the binding would point at freed memory for the rest of the block. Move the clear out of the block.",
                    "with xs[0] as e {\n    xs.clear()               // error: clear invalidates all elements\n}"),
                "E0862" => ("a closure holding a scoped borrow can't escape", Ownership,
                    "A closure that captures a block-scoped borrow lives as long as that block and no longer. Returning it, or storing it somewhere that outlives the block, would leave it holding a reference to something already gone.\n\n`own ||` is the escape hatch: it moves what it captures instead of borrowing, so the closure owns everything it needs.",
                    "with data.read() as d {\n    return || { d.len() }    // error: closure would outlive the borrow\n}\n// fix: move the captures\nreturn own || { d.len() }"),
                "E0852" => ("a generic method can't be called through a trait object", Trait,
                    "`any Trait` erases the concrete type, and a generic method needs one — each instantiation is separate code, and there is nothing left to pick which. Call it on the concrete type instead (TR3).",
                    "func run(x: any Shape) { x.scale<f32>(2.0) }   // error: generic method\n// fix: take the concrete type\nfunc run<S: Shape>(x: S) { x.scale<f32>(2.0) }"),
                "E0853" => ("`to_map` needs pairs", Type,
                    "`to_map` turns a sequence of `(K, V)` tuples into a map. A sequence of anything else has no key to put things under — produce the pairs first.",
                    "users.to_map()               // error: a sequence of `User`\n// fix: say what the key is\nusers.map(|u| (u.id, u)).to_map()"),
                "E0854" => ("annotation used wrongly", Type,
                    "The annotation is real but this use of it isn't — a missing argument, an argument of the wrong shape, or an attachment point it doesn't cover. The message names which. An annotation the compiler can't act on is worse than one it rejects, because the source would say something the program doesn't do.",
                    "@tag struct Msg { … }        // error: `@tag` needs a name\n// fix: give it one\n@tag(\"kind\") struct Msg { … }"),
                "E0856" => ("package-level state written without a sync box", Type,
                    "A module-level `const` is one instance for the whole program, reachable from every task. Writing to a bare one is a data race that doesn\'t announce itself — a `Vec` hammered from two threads loses updates and corrupts the heap. PS2 puts package-level mutable state behind `Shared`, `Shared.mutex` or `Atomic`; PS3 is why there is no `mut` at package level to reach for instead.",
                    "const NAMES: Vec<string> = Vec.new()\nNAMES.push(n)                // error: needs a sync box\n// fix: give it a lock\nconst NAMES = Shared.new(Vec.new())\nwith NAMES.write() as v { v.push(n) }"),
                "E0857" => ("a pointer's element type doesn't match the slot's", Type,
                    "A pointer is an address, and whoever reads through it picks the stride from its own element type. So `*i64` and `*i32` are different types with no conversion between them — passing one where the other is declared doesn\'t reinterpret anything, it just makes the two ends disagree about how far apart the elements are.\n\nThis is the shape `import c` produces most often: a header\'s `int` is `c_int`, which is 32-bit, and a `Vec<i64>` buffer handed to it reads as twice as many half-width numbers.",
                    "c.sum(v.as_ptr(), 3)         // error: `*i64` where `*i32` is declared\n// fix: build it at the width the C side reads\nmut v: Vec<i32> = Vec.new()"),
                "E0858" => ("a C function returns a struct by value", Type,
                    "Handing a struct *to* a C function works — it goes in registers, or on the stack when it is too big. Getting one *back* is a different ABI rule, and it isn't built yet, so the compiler rejects the call rather than reading back a value nobody wrote.\n\nAn out-parameter is the way through: the C side takes `Rect *out` and writes into a struct you already own.",
                    "let r = c.make_rect(3, 4)    // error: returns `c.Rect` by value\n// fix: hand it somewhere to write\nmut r = c.Rect { width: 0, height: 0 }\nc.fill_rect(&r, 3, 4)"),
                "E0855" => ("`@allow` names nothing", Type,
                    "`@allow(...)` takes one compiler warning name or one lint rule id. A name that matches neither suppresses nothing, and the warning fires as if the annotation weren't there — which reads exactly like a warning you suppressed correctly that later stopped firing on its own. So a name nothing answers to is an error.",
                    "@allow(torn_lock_updat)      // error: names nothing\n// fix: spell it out\n@allow(torn_lock_update)"),
                "E0807" => ("a resource consumed twice", Ownership,
                    "Linearity: a `@resource` is consumed exactly once. The second use is a use of something that no longer exists — a file closed twice, a transaction committed and then rolled back.\n\nThe message points at both places, so the one to delete is usually obvious.",
                    "file.close()\nfile.close()                 // error: `file` already consumed"),
                "E0810" => ("a captured resource isn't consumed on every path", Ownership,
                    "A resource captured by a closure or a task is that body's to finish with, and \"exactly once\" has to hold on every path through it — including the ones that return early or raise.\n\n`ensure` at the top of the body is the usual answer: it runs at every exit, including a panic.",
                    "spawn(own || {\n    if bad { return }        // error: `conn` not consumed here\n    conn.close()\n})\n// fix: one exit for all paths\nspawn(own || { ensure conn.close(); … })"),
                "R0001" => ("division by zero", Runtime,
                    "Integer division and remainder by zero have no answer, so the program stops rather than continuing with a number nobody chose. Check the divisor first, or use a form that hands back an absence.\n\nThe same check at compile time reports this code too: a `comptime` block that divides by zero fails the fold with it.",
                    "let avg = total / count      // panics when `count` is 0\n// fix: decide what zero means here\nlet avg = if count == 0 { 0 } else { total / count }"),
                "R0002" => ("index out of bounds", Runtime,
                    "Every index into a Vec, an array or a string is range-checked at the access (std.collections/V1). A negative or too-large index panics — there is no wraparound and no negative-from-end indexing, because both turn a bug into a different value rather than into a stop.\n\n`get` is the form that answers `T?` instead of panicking.",
                    "let x = xs[i]                // panics when `i >= xs.len()`\n// fix: ask instead of assume\nlet x = xs.get(i) ?? default"),
                "R0003" => ("variable not found at run time", Runtime,
                    "The interpreter reached a name that isn't bound. This is almost always a compiler bug rather than a program one — an undefined name is E0341 at check time — so it usually means a lowering or scoping path let something through.",
                    "// no user-level fix: report it with the program that produced it"),
                "R0004" => ("function not found at run time", Runtime,
                    "A call reached a function the interpreter doesn't have. Check the spelling and the import, but if the name is a stdlib one this usually means the backend hasn't implemented it — a declaration marked `@unimplemented`, or one whose native symbol only exists on the other backend.",
                    "os.signals()                 // this backend has no implementation\n// fix: run it natively, or use a built alternative"),
                "R0005" => ("type error at run time", Runtime,
                    "A value turned out not to be the shape the operation needed. Most of these are caught at check time, so one arriving here usually means a type the checker left open — an inference variable that reached the interpreter as a guess.\n\nAn annotation on the binding is the usual fix, and worth reporting either way.",
                    "let xs = Vec.new()           // element type never settles\n// fix: say what it holds\nlet xs: Vec<i64> = Vec.new()"),
                "R0006" => ("wrong number of arguments", Runtime,
                    "A call reached the interpreter with an argument count the function doesn't take. Arity is checked at compile time (E0310), so this usually means a call built by the compiler itself — a desugaring or a generated method.",
                    "// no user-level fix: report it with the program that produced it"),
                "R0007" => ("no such method at run time", Runtime,
                    "The receiver has no method by this name. Method resolution happens at check time (E0313), so reaching here means the receiver's type was still open when it was checked — the call was deferred and the type it settled on has no such method.",
                    "let v = load()               // return type never settles\nv.push(1)                    // no `push` on what it became\n// fix: annotate the binding"),
                "R0008" => ("no such field at run time", Runtime,
                    "The value has no field by this name. Like R0007, this is the deferred half of a check that normally happens at compile time (E0312).",
                    "// annotate the binding whose type stayed open"),
                "R0009" => ("a closed resource was used", Runtime,
                    "A `@resource` is consumed exactly once, and the operations on it stop working after that. The compiler proves this for a value it can follow (E0807), so one arriving here got past it — usually through a container, a closure capture, or a dynamic path.",
                    "file.close()\nfile.write(\"x\")              // the handle is spent\n// fix: order the uses, or reopen"),
                "R0010" => ("panic", Runtime,
                    "Something called `panic(…)`, or a check the runtime performs failed and reported itself as one. The task unwinds: every `ensure` on the way out runs, locks release without poisoning, and the process exits 101 (ctrl.panic).\n\nThe message is the program's own, so what to do about it depends on what raised it.",
                    "panic(\"unreachable state: {tag}\")"),
                "R0011" => ("no arm matched", Runtime,
                    "A `match` reached a value none of its arms cover. Exhaustiveness is checked at compile time (E0340), so this means the scrutinee held something the checker didn't know it could — usually an integer-backed enum decoded from outside the program.",
                    "match tag_from_wire() {\n    Ok => …\n    Err => …                 // and the wire said 7\n}\n// fix: cover the rest\n_ => return DecodeError.UnknownTag"),
                "R0012" => ("more than one entry point", Runtime,
                    "A program has exactly one place to start. Both a `func main()` and an `@entry` function, or two `@entry` functions, leave nothing to pick between.",
                    "func main() { … }\n@entry func start() { … }    // two entry points\n// fix: keep one"),
                "R0013" => ("no entry point", Runtime,
                    "Nothing in the program says where to start. Add `func main()`, or mark a function `@entry`.\n\nA library doesn't need one — this is only an error for something being run.",
                    "// fix: give it a start\nfunc main() { … }"),
                "R0014" => ("assertion failed", Runtime,
                    "An `assert` found its condition false. The message shows both operands where the assertion was a comparison, so the two values are in front of you rather than one line up.\n\nAsserts are on in every build: an invariant worth writing down is worth checking where it matters.",
                    "assert total == expected\n// assertion failed: 41 == 42 (left: 41, right: 42)"),
                "R0015" => ("check failed", Runtime,
                    "A `check` found its condition false. Unlike `assert`, a failed `check` records the failure and lets the test carry on, so one run reports every one it finds instead of stopping at the first.",
                    "check a == 1\ncheck b == 2                 // both are reported"),
                "R0016" => ("`!` on a value that was absent", Runtime,
                    "`!` takes the payload of a `T?` and panics when there isn't one (type.optionals/OPT13). That's the point of the spelling — it's the short way to say \"I know this is here\", and it's loud when you were wrong.\n\n`??` substitutes a value instead, and `x is T as v` tests for one first.",
                    "let user = find(id)!         // panics when there's no such user\n// fix: say what happens when it's absent\nlet user = find(id) ?? guest()"),
                "R0017" => ("runtime error", Runtime,
                    "A runtime failure with no more specific code — the message carries what happened. If it reads like a compile error, it is one that reached the interpreter instead of the checker, and is worth reporting.",
                    "// no fixed shape: read the message"),
                "R0018" => ("arithmetic overflowed", Runtime,
                    "Arithmetic panics on overflow in every build, release included (type.overflow/OV1). A number that doesn't fit is a bug, and the alternatives — wrapping silently, or being undefined — both turn it into a wrong answer somewhere else.\n\nWhere wrapping *is* the intent, `Wrapping<T>` from `num` says so. Where it might not fit, `checked_add` and its siblings answer `T?`.\n\nThe same check at compile time reports this code too: a `comptime` block that overflows fails the fold with it.",
                    "let n = a + b                // panics when it doesn't fit\n// fix: say which\nlet n = a.checked_add(b) ?? i64.MAX"),
                "R0019" => ("`!` on a value that was an error", Runtime,
                    "`!` takes the ok payload of a `T or E` and panics on the error branch, using the error's own `message()` (type.errors/ER15). So the panic says what went wrong rather than just that something did.\n\n`try` sends the error to the caller instead, and `catch e =>` handles it here.",
                    "let cfg = load(path)!        // panics with the error's message\n// fix: propagate or handle\nlet cfg = try load(path)"),
                "R0022" => ("main returned an error", Runtime,
                    "`main` can return `T or E`, and an error out of it is a failed run: the message is printed and the process exits 1 (struct.targets/EX4). That's the whole mechanism — there is no separate exit-code plumbing to write.",
                    "func main() -> () or IoError {\n    try run()\n}\n// an error here prints its message and exits 1"),
                "R0023" => ("recursion too deep", Runtime,
                    "The interpreter spends one host stack frame per Rask call and those frames are large, so it moves onto a fresh stack every few hundred calls rather than overflowing. That chain is capped — around a gigabyte of live stack — so a recursion that never terminates stops here with a message instead of taking the machine down.\n\nCheck the base case if this was meant to terminate. Otherwise rewrite it as a loop, or run it natively with `rask run`, which has no such limit.",
                    "func depth(n: i64) -> i64 { return depth(n + 1) }   // never terminates\n// fix: give it a base case"),
                "W0301" => ("`discard` on a Copy type frees nothing", Type,
                    "`discard` exists to end a value\'s life before its scope does — to release the memory it owns at a point you choose rather than at the closing brace. A Copy type owns no memory, so there is nothing to release and the statement reads as a cost it doesn\'t pay (mem.ownership/D2).\n\nIt does still put the name out of use, which is D1 and holds for every type. If that was the point, a comment says so more clearly than a `discard` that looks like cleanup.",
                    "let n = 7\ndiscard n            // warning: frees nothing\n// fix: drop the line\nlet n = 7"),
                "W0303" => ("comptime const could not be folded, so it runs at runtime", Type,
                    "The comptime evaluator doesn't cover the whole language yet, and this block reached a corner it can't model — a static method it has no implementation for, a value it can't represent. The program still works: the block is evaluated at startup instead. What's lost is the guarantee `comptime` was written for, so this is worth knowing about rather than silent. The warning names what stopped it.",
                    "const SPRITES = comptime {\n    mut v = Vec.new()\n    v.push(load_atlas())        // warning: I/O isn't available at comptime\n    v.freeze()\n}"),
                "W0302" => ("range step runs the wrong way, so the range is empty", Type,
                    "A positive step on a descending range, or a negative step on an ascending one, never reaches the far end — the loop body runs zero times (ctrl.ranges/SP1-SP2). That is legal and almost never intended, so it's a warning rather than an error. Match the step's sign to the range's direction, or swap the endpoints.",
                    "for i in (10..0).step(1) { … }   // warning: runs zero times\n// fix: descend\nfor i in (10..0).step(-1) { … }\n// or ascend\nfor i in (0..10).step(1) { … }"),
            },
        }
    }
}

impl ErrorCodeRegistry {
    pub fn get(&self, code: &str) -> Option<&ErrorCodeInfo> {
        self.codes.get(code)
    }

    pub fn all(&self) -> impl Iterator<Item = &ErrorCodeInfo> {
        self.codes.values()
    }
}

// ─── Registry audit ────────────────────────────────────────
// The registry and the emitters are two hand-maintained lists that have to
// agree, and nothing made them. `register_codes!` builds a HashMap, so a
// duplicate code is a silent overwrite rather than an `unreachable_patterns`
// warning — E0831 was registered twice and the later entry won, so anyone who
// hit "`??` on a value that is always there" and ran `rask explain E0831` got
// an explanation about `using` clauses on `main` instead (#892).
//
// These read `convert.rs` and this file as text, so they're derived from what
// the compiler actually does rather than from a list someone has to remember
// to update.
#[cfg(test)]
mod registry_audit {
    use super::ErrorCodeRegistry;

    const CONVERT_RS: &str = include_str!("convert.rs");
    const CODES_RS: &str = include_str!("codes.rs");

    /// Every `with_code("…")` in `convert.rs`, with the match arm it sits under.
    ///
    /// A `with_code` inside a free helper function belongs to that helper, not
    /// to whichever arm happened to be last: attributing one to the arm above
    /// it reported E0328 as shared with `ConflictingMethods`, which never
    /// emitted it (#992).
    fn emitted_codes() -> Vec<(String, String, usize)> {
        let mut out = Vec::new();
        let mut arm = String::from("<none>");
        for (n, line) in CONVERT_RS.lines().enumerate() {
            let indent = line.len() - line.trim_start().len();
            let head = line.trim_start();
            if head.starts_with("fn ")
                || head.starts_with("pub fn ")
                || head.starts_with("pub(crate) fn ")
                || head.starts_with("pub(super) fn ")
            {
                let name: String = head
                    .rsplit_once("fn ")
                    .map(|(_, r)| r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect())
                    .unwrap_or_default();
                arm = format!("fn {name}");
            }
            // Match arms sit at 8–16 spaces and start with the variant name.
            if (8..=16).contains(&indent) {
                let name: String = head
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let rest = head[name.len()..].trim_start();
                if name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                    && (rest.starts_with('{') || rest.starts_with("=>") || rest.starts_with('('))
                {
                    arm = name;
                }
            }
            let mut rest = line;
            while let Some(i) = rest.find("with_code(\"") {
                rest = &rest[i + "with_code(\"".len()..];
                if let Some(end) = rest.find('"') {
                    let code = &rest[..end];
                    // Runtime codes too. `RuntimeDiagnostic` has its own R00xx
                    // namespace and the registry never covered it, so `rask
                    // explain R0001` said the code didn't exist — the same
                    // wrong answer a shared code gives, arrived at by a
                    // different route (#992). A reader looking a code up off a
                    // panic has the question a compile error's reader has.
                    if matches!(code.as_bytes()[0], b'E' | b'W' | b'R') {
                        out.push((code.to_string(), arm.clone(), n + 1));
                    }
                }
            }
        }
        out
    }

    /// Every code literal registered in this file, in source order.
    fn registered_codes() -> Vec<String> {
        CODES_RS
            .lines()
            .filter_map(|l| {
                let l = l.trim_start();
                let rest = l.strip_prefix('"')?;
                let end = rest.find('"')?;
                let code = &rest[..end];
                if rest[end + 1..].trim_start().starts_with("=>")
                    && code.len() == 5
                    && matches!(code.as_bytes()[0], b'E' | b'W' | b'R')
                    && code[1..].bytes().all(|b| b.is_ascii_digit())
                {
                    Some(code.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn no_code_is_registered_twice() {
        let all = registered_codes();
        let mut seen = std::collections::HashMap::new();
        let mut dupes = Vec::new();
        for c in &all {
            if seen.insert(c.clone(), ()).is_some() {
                dupes.push(c.clone());
            }
        }
        assert!(
            dupes.is_empty(),
            "codes.rs registers these twice, so only the last entry survives \
             the HashMap and `rask explain` answers about the wrong error: {:?}",
            dupes
        );
    }

    #[test]
    fn registry_entries_are_reachable_by_get() {
        // Guards the failure mode above from the other side: the parsed source
        // and the built map must hold the same number of codes.
        let registry = ErrorCodeRegistry::default();
        assert_eq!(
            registry.all().count(),
            registered_codes().len(),
            "codes.rs has more entries than the built registry — some code is \
             registered twice and one entry was overwritten"
        );
    }

    /// Codes emitted by `convert.rs` with no `rask explain` entry.
    ///
    /// Every one of these is a user who reads a code off a diagnostic, asks the
    /// compiler what it means, and is told the code doesn't exist. This list may
    /// shrink, never grow. Two reasons appear here, and both clear once the
    /// underlying problem is fixed rather than by adding text:
    ///
    ///  - Shared codes (in SHARED_CODES below). An entry could only describe one
    ///    of the two errors, so the other meaning would get a confidently wrong
    ///    answer — which is exactly the E0831 bug this module exists to prevent.
    ///    Saying "unknown code" is the honest answer until they're renumbered.
    ///  - Match arms that are declared and formatted but never constructed.
    ///    Unreachable today, so there is no error to explain (#992).
    /// Empty, and it should stay that way: every code a program can produce can
    /// be looked up. The three that used to sit here were arms nobody could
    /// reach — two deleted (`MissingMutateAnnotation`, superseded by E0373, and
    /// `MessageCoverageMissing`, whose rule ER38 the derived `message()` covers)
    /// and one wired up (`DiscardCopyType`, mem.ownership/D2).
    const UNEXPLAINED: &[&str] = &[];

    #[test]
    fn every_emitted_code_can_be_explained() {
        let registry = ErrorCodeRegistry::default();
        let mut missing: Vec<String> = emitted_codes()
            .into_iter()
            .filter(|(code, _, _)| {
                registry.get(code).is_none() && !UNEXPLAINED.contains(&code.as_str())
            })
            .map(|(code, arm, line)| format!("{} ({} at convert.rs:{})", code, arm, line))
            .collect();
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "these codes are emitted but `rask explain` doesn't know them — \
             add an entry to codes.rs: {:#?}",
            missing
        );
    }

    /// Codes that two or more genuinely different errors both report.
    ///
    /// A code is supposed to identify one error, so a shared code makes
    /// `rask explain` wrong for every meaning but one, and makes the code
    /// useless for searching. Thirty-one codes were shared; twenty-nine were
    /// split in #992. Which meaning kept the number was decided in this order:
    /// a citation by name in `specs/` pins it, otherwise the meaning the
    /// registry entry already described, otherwise the one a user hits most.
    ///
    /// The two left are one error each, spelled two ways for the reader's
    /// sake: a resource that wasn't consumed, named or opaque, and a borrowed
    /// parameter given away, phrased as a consume or as a move. Splitting them
    /// would make `rask explain` answer half a question. Pinned here so the
    /// count can only go down.
    const SHARED_CODES: &[&str] = &["E0805", "E0806"];

    #[test]
    fn no_new_code_serves_two_errors() {
        let mut by_code: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            Default::default();
        for (code, arm, _) in emitted_codes() {
            by_code.entry(code).or_default().insert(arm);
        }
        let new: Vec<String> = by_code
            .into_iter()
            .filter(|(code, arms)| arms.len() > 1 && !SHARED_CODES.contains(&code.as_str()))
            .map(|(code, arms)| {
                format!("{} → {:?}", code, arms.into_iter().collect::<Vec<_>>())
            })
            .collect();
        assert!(
            new.is_empty(),
            "these codes each report two or more different errors, so \
             `rask explain` can only describe one of them. Give the new error \
             its own code: {:#?}",
            new
        );
    }
}
