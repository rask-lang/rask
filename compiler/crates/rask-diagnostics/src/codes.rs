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
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCategory::Syntax => write!(f, "Syntax"),
            ErrorCategory::Resolution => write!(f, "Resolution"),
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
                    "// Error: unexpected '@' (outside attribute position)\nconst x = @value"),
                "E0002" => ("unterminated string literal", Syntax,
                    "A string was opened with `\"` but never closed. Every string literal must have a matching closing quote on the same line.",
                    "// Error: string never closed\nconst msg = \"hello world"),
                "E0003" => ("invalid escape sequence", Syntax,
                    "A backslash in a string was followed by a character that isn't a recognized escape. Valid escapes: \\n, \\t, \\r, \\\\, \\\", \\0.",
                    "// Error: \\q is not a valid escape\nconst s = \"path\\qname\""),
                "E0004" => ("invalid number format", Syntax,
                    "A numeric literal has an invalid format — perhaps a suffix typo, multiple dots, or an invalid digit for the base.",
                    "// Error: invalid suffix\nconst x = 42i3  // did you mean i32?"),

                // Parser errors (E01xx)
                "E0100" => ("unexpected token", Syntax,
                    "The parser encountered a token that doesn't make sense in the current context. Check for missing operators, mismatched brackets, or Rust syntax habits.",
                    "// Error: unexpected '::'\nconst x = Option::Some(1)  // use Option.Some(1)"),
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
                    "import math\nconst math = 42  // error: shadows import"),
                "E0209" => ("shadows built-in", Resolution,
                    "A definition has the same name as a built-in type or function. This can cause confusing errors later. Choose a different name.",
                    "struct Vec { }  // error: shadows built-in Vec"),

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
                    "const x = 42\nx()  // error: i64 is not callable"),
                "E0312" => ("no such field", Type,
                    "The struct doesn't have a field with this name. Check the struct definition for available fields.",
                    "struct Point { x: i32, y: i32 }\nconst p = Point { x: 1, y: 2 }\np.z  // error: no field 'z' on Point"),
                "E0313" => ("no such method", Type,
                    "The type doesn't have a method with this name. Check the extend blocks for available methods, or verify the receiver type.",
                    "const v = Vec.new()\nv.length()  // error: did you mean v.len()?"),
                "E0314" => ("infinite type", Type,
                    "A type would need to contain itself without indirection, creating an infinite-size type. Use `Owned<T>` for indirection.",
                    "struct Node {\n    next: Node  // error: infinite size\n    // fix: next: Owned<Node>?\n}"),
                "E0315" => ("cannot infer type", Type,
                    "The compiler can't determine a type from context alone. Add an explicit type annotation.",
                    "const x = Vec.new()  // error: Vec of what?\n// fix: const x: Vec<i32> = Vec.new()"),
                "E0316" => ("invalid try context", Type,
                    "`try` propagates errors to the caller, so the enclosing function must return `T or E` (a Result type).",
                    "func f() {\n    const x = try might_fail()  // error: f() doesn't return Result\n}"),
                "E0317" => ("try outside function", Type,
                    "`try` can only appear inside a function body. It needs a function return type to propagate errors to.",
                    "const x = try some_call()  // error: not in a function"),
                "E0318" => ("missing return statement", Type,
                    "A function with a non-void return type doesn't return a value on all paths. Rask requires explicit `return` in functions.",
                    "func double(x: i32) -> i32 {\n    x * 2  // error: missing 'return'\n    // fix: return x * 2\n}"),
                "E0319" => ("generic argument error", Type,
                    "A generic type was instantiated with the wrong number or kind of arguments.",
                    "// Vec takes 1 type param\nconst x: Vec<i32, string> = Vec.new()  // error"),
                "E0320" => ("aliasing violation", Type,
                    "A value was mutated while it's being borrowed. Finish using the borrow before mutating, or clone the value.",
                    "const v = Vec.new()\nconst first = v[0]  // borrows v\nv.push(4)  // error: v is borrowed"),
                "E0321" => ("mutate read-only parameter", Type,
                    "Parameters are read-only by default in Rask. To modify a parameter, add the `mutate` keyword.",
                    "func reset(v: Vec<i32>) {\n    v.clear()  // error: v is read-only\n}\n// fix: func reset(mutate v: Vec<i32>)"),
                "E0322" => ("volatile view stored", Type,
                    "A view (reference) into a growable collection was stored across a statement boundary. Views into Vec, Pool, and Map are instant — they're released at the semicolon.",
                    "const v = Vec.new()\nconst elem = v[0]  // view into v\n// elem is invalid after this line if v changes"),
                "E0323" => ("mutate while viewed", Type,
                    "A collection was mutated while a view into it exists. This could invalidate the view. Finish using the view first.",
                    "const v = Vec.new()\nconst elem = v[0]\nv.push(4)  // error: v viewed by elem"),
                "E0324" => ("heap allocation in @no_alloc function", Type,
                    "@no_alloc functions run in real-time contexts where heap allocation causes unpredictable latency. Use stack-allocated alternatives or pre-allocated buffers.",
                    "@no_alloc\nfunc process(data: [f32; 64]) {\n    const v = Vec.new()  // error: allocates\n}"),
                "E0325" => ("write in frozen pool context", Type,
                    "A `using frozen Pool<T>` context is read-only (mem.pools/PF5): no writes through handles, and no insert/remove/clear. Drop `frozen` if the function needs to mutate the pool.",
                    "func heal(h: Handle<Player>) using frozen Pool<Player> {\n    h.health += 10  // error: frozen context\n}\n// fix: using Pool<Player>  (drop `frozen`)"),
                "E0342" => ("unknown context", Type,
                    "A `using` block references a context that doesn't exist. Valid contexts are `Multitasking` and `ThreadPool`.",
                    "using Foo {\n    // error: unknown context `Foo`\n}"),
                "E0345" => ("type name called as a function", Type,
                    "`Name(value)` is the constructor for a nominal type declared with `type Name = Underlying` (T7). Structs have named fields and no tuple form (S1), so they're built with a literal; enums are named by variant.",
                    "struct TaskId { public value: u64 }\n\nconst a = TaskId(1)             // error\nconst b = TaskId { value: 1 }   // fix"),
                "E0351" => ("runtime context on signature", Type,
                    "`using Multitasking` and `using ThreadPool` install a process-global runtime slot. They cannot appear on function signatures — only on block expressions.",
                    "// Error: signature-level using is not allowed\nfunc run_tasks() using Multitasking { }\n\n// Fix: wrap the call site instead\nfunc main() {\n    using Multitasking {\n        run_tasks()\n    }\n}"),
                "E0352" => ("spawn outside multitasking block", Type,
                    "`spawn` requires an active `using Multitasking { ... }` block to be in scope. Without a runtime, there's nowhere to submit the task.",
                    "func main() {\n    spawn { do_work() }  // error: no `using Multitasking` block\n\n    // Fix:\n    using Multitasking {\n        spawn { do_work() }  // ok\n    }\n}"),
                "E0353" => ("transitive spawn outside multitasking block", Type,
                    "A function that transitively calls `spawn` is being called without an active `using Multitasking { ... }` runtime scope. The call will panic at runtime.",
                    "func do_work() {\n    spawn(|| { task() })  // reaches spawn\n}\n\nfunc main() {\n    do_work()  // error: no runtime scope\n\n    // Fix:\n    using Multitasking {\n        do_work()  // ok\n    }\n}"),
                "E0354" => ("duplicate variant in sum type", Type,
                    "A sum type cannot contain the same payload variant twice — `(T or E) or E` is ambiguous, because the compiler picks the branch from the value's type and an `E` value fits both. `none` is exempt: `T??` is a legal two-layer optional whose layers stay distinct. Use a named enum if you need two flavours of the same error.",
                    "// Error: duplicate `ParseError` branch\nfunc f() -> (i32 or ParseError) or ParseError { }\n\n// Fix: use a named enum\nenum LookupResult { Found(User), Missing, Forbidden }\nconst x: LookupResult = LookupResult.Missing"),
                "E0358" => ("generic instantiation collapses `T or E`", Type,
                    "A generic returning `T or E` was called with a type argument equal to `E`. Both branches would then carry the same type and the caller could not tell a success from an error. The signature's `T or E` is itself the requirement that they stay distinct — it is checked here, at the call, where the type argument is known. Newtype one side to keep them apart.",
                    "enum CacheError { Miss }\n\nfunc cached<T>(v: T) -> T or CacheError { return v }\n\nfunc main() {\n    const ok = cached(42)                 // T = i32: fine\n    const bad = cached(CacheError.Miss)   // error: T = CacheError\n\n    // Fix: newtype the success side\n    type Cached = CacheError with (…)\n}"),
                "E0356" => ("unknown type in signature", Type,
                    "A PascalCase name in a function signature doesn't resolve to any declared type. Only single uppercase letters (T, U, K, V) are auto-generic type parameters — longer names must be declared types. This catches typos early instead of silently treating them as generics.",
                    "struct Config { port: i32 }\nfunc load(c: Confg) { }  // error: did you mean `Config`?\n\n// Auto-generic still works with single letters:\nfunc swap(a: T, b: T) -> (T, T) { return (b, a) }"),
                "E0357" => ("single-letter type name", Type,
                    "Single uppercase letters are reserved for type parameters. A struct, enum, trait, or union named `T` would be shadowed by the type-parameter convention in every signature.",
                    "struct T { }  // error: reserved for type parameters\n// fix: struct Token { }"),
                "E0355" => ("error type mismatch in try", Type,
                    "`try` propagates the inner error to the enclosing function, so the two error types have to line up. They line up three ways: the same type, a member of the function's error union, or a single-payload variant of the function's error enum. Anything else needs an explicit map at the call — `try expr else |e| …`.",
                    "enum ParseError { Syntax(string) }\nenum ApiError { Parse(ParseError), BadRequest(string) }\n\nfunc inner() -> i32 or ParseError { return 42 }\n\nfunc outer() -> i32 or ApiError {\n    const x = try inner()  // ok: ApiError.Parse takes a single ParseError\n    return x\n}"),
                "E0359" => ("ambiguous error wrap in try", Type,
                    "Two or more variants of the function's error enum take the propagated error as their only payload, so `try` has no way to choose. Name the variant at the call site.",
                    "enum StoreError { NotFound(string) }\nenum ApiError { Store(StoreError), Fatal(StoreError) }\n\nfunc outer() -> i32 or ApiError {\n    const x = try lookup()  // error: Store or Fatal?\n\n    // Fix: say which\n    const y = try lookup() else |e| ApiError.Store(e)\n    return y\n}"),

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
                    "const v = Vec.new()\ntake_ownership(own v)\nv.len()  // error: v was moved"),
                "E0801" => ("borrow conflict", Ownership,
                    "Multiple borrows conflict — typically a mutable borrow while an immutable borrow exists.",
                    "let v = Vec.new()\nconst first = v[0]  // immutable borrow\nv.push(4)  // error: mutable borrow conflicts"),
                "E0802" => ("mutate while borrowed", Ownership,
                    "A value was mutated while it's borrowed. The borrow must end before mutation is allowed.",
                    "let s = \"hello\"\nconst r = s\ns = \"world\"  // error: s is borrowed by r"),
                "E0803" => ("instant borrow escapes", Ownership,
                    "A reference from a collection access was stored past its valid scope. Collection references are instant — valid for one expression only.",
                    "const v = Vec.new()\nlet elem = v[0]\n// elem may be invalid if v reallocates"),
                "E0804" => ("borrow escapes scope", Ownership,
                    "A reference outlives the value it borrows from. The borrowed value must live at least as long as the reference.",
                    "func bad() -> string {\n    const local = \"temp\"\n    return local  // error if local is stack-allocated\n}"),
                "E0805" => ("resource not consumed", Ownership,
                    "A resource-typed value (marked with @resource) wasn't properly consumed. Resource types must be explicitly closed, released, or passed to a consuming function.",
                    "func open_file() {\n    const f = fs.open(\"data.txt\")\n    // error: f not consumed (must call f.close())\n}"),
                "E0806" => ("move from borrowed parameter", Ownership,
                    "A borrowed parameter was used in a context that requires ownership. Parameters are borrowed by default — the caller retains ownership. Use `take` to transfer ownership into the function.",
                    "func push(self, value: T) {\n    self.data[i] = value  // error: value is borrowed\n}\n// fix: func push(self, take value: T)"),
                "E0811" => ("use after discard", Ownership,
                    "`discard` explicitly drops a value and invalidates its binding. Using the binding after `discard` is a compile error (D1).",
                    "const data = load_data()\ndiscard data\nprintln(data)  // error: use of discarded value"),
                "E0812" => ("discard resource type", Ownership,
                    "Resource types (@resource) must be consumed properly via their consuming method (.close(), .release(), etc). `discard` on a resource type is a compile error (D3).",
                    "@resource\nstruct File { fd: i32 }\nconst f = File { fd: 1 }\ndiscard f  // error: use f.close() instead"),
                "E0813" => ("use after maybe-move", Ownership,
                    "A value moved on some paths but not all (e.g. one `if` branch) was used after the paths merged. The spec treats maybe-moved as moved (O3) — move on every path, or keep the use inside the branch that still owns the value.",
                    "const v = Vec.new()\nif c { take(own v) }\nv.len()  // error: v may have been moved"),
                "E0817" => ("invalid `as` cast", Type,
                    "`as` permits only lossless widening (CV1). Narrowing, sign reinterpretation, float↔int, int→char, and int↔bool are compile errors — use the explicit conversion forms (`truncate to`, `saturate to`, `try convert to`, `float to int`) or `char.from_u32`.",
                    "const x: i8 = big as i8  // error: use `big truncate to i8`"),
                "E0818" => ("invalid conversion form", Type,
                    "A CV5–CV10 conversion form was applied to the wrong source/target kind — e.g. `float to int` on an integer, or `truncate to` producing a non-integer.",
                    "const x = n float to int i32  // error if n is already an integer"),
                "E0819" => ("index type mismatch", Type,
                    "An index expression `c[i]` used the wrong index type. Vec, arrays, slices, and strings are position-indexed by an integer; `Map<K,V>` is indexed by `K`; `Pool<T>` is indexed by `Handle<T>`. Range indexing (slicing) only works on Vec, arrays, slices, and strings.",
                    "const s = \"hi\"\nv[s]  // error: index a Vec with an integer, not a string"),
                "E0820" => ("linear value in container", Ownership,
                    "A Vec or Map element (or Map key) is a linear value — an @resource type, a transitively-linear struct/enum, or an optional/tuple/array built from one. Vec/Map drop can't consume linear elements, so they'd be silently dropped (RC1/RC3). Use `Pool<T>` (explicit removal, RC2) or `T?` (match and consume, RC4).",
                    "@resource\nstruct File { fd: i32 }\nconst files: Vec<File> = Vec.new()  // error: use Pool<File>"),
                "E0821" => ("ensure receiver maybe-consumed", Ownership,
                    "A resource with a pending `ensure` was consumed on some paths but not all, and the paths merge before scope exit (C4). Which cleanup runs must be statically definite — never decided by hidden runtime state (C3). Exit inside the consuming branch, or consume on every path.",
                    "const tx = try db.begin()\nensure tx.rollback()\nif fast { tx.commit() }  // error: paths merge with tx maybe-consumed\nlog(\"done\")"),
                "E0822" => ("missing struct field", Type,
                    "A struct literal left out a field that has no default value. Construction never zero-initializes — a defaultless field must be given a value, or declared with a default (`field: T = value`). A spread (`..base`) supplies every unlisted field.",
                    "struct Config { host: string, port: i32 = 8080 }\nconst c = Config {}  // error: missing field `host`"),
                "E0823" => ("method name shared by two types", Type,
                    "Two different types have the same name — usually a program type and a stdlib one — and both define this method. Compiled functions are identified by `Type_method`, so the two methods want the same name and only one can have it. The type the name currently refers to gets it; a call needing the other has nowhere to go. Rename one of the types.",
                    "// stdlib already has `enum JsonError` with `message()`\nstruct JsonError { detail: string }\nextend JsonError {\n    func message(self) -> string { return self.detail }\n}\n// fix: name it something else, e.g. `ConfigJsonError`"),
                "E0824" => ("public duck trait", Type,
                    "A `duck trait` was declared `public`. Duck traits match by shape instead of by declaration, which makes them a versioning trap across a package boundary: an external type could start or stop satisfying the trait because its author added or removed a method, with nothing in either diff to notice. So duck traits stay package-internal (type.generics/DT1) — they're for code you're still sketching. Drop `duck` to harden the trait (the compiler generates the conformance declarations for types that already match), or drop `public`.",
                    "public duck trait Frobber {\n    func frobnicate(self)\n}\n// fix: `public trait Frobber` (nominal), or `duck trait Frobber` (package-internal)"),
                "E0825" => ("integer literal out of range", Type,
                    "An integer literal doesn't fit the type it ended up with. Unsuffixed literals are `i32` by default (type.primitives/L1) and widen to `i64` when the value needs it; a literal that reaches a narrower type through an annotation, a suffix, or a parameter has to fit that type. Nothing wraps silently — pick a wider type, or convert at the use site.",
                    "const b: u8 = 300  // error: 300 doesn't fit u8 (0..=255)\n// fix: `const b: u16 = 300`, or `const b = 300 truncate to u8`"),
                "E0826" => ("type does not implement Displayable", Type,
                    "`{}` in a format template calls `to_string()`, which comes from `Displayable` (std.fmt/D4). Primitives have it; structs and enums opt in with `extend Type with Displayable`, and error types get it for free from `message()` (D5). Optionals and results are never Displayable — an optional may have nothing to show, so the missing case has to be spelled out at the call.",
                    "const found: User? = lookup(id)\nprintln(\"{found}\")   // error: `User?` has no to_string()\n// fix: `println(\"{found ?? \\\"nobody\\\"}\")`, or narrow first with `if found? as u { … }`"),
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
