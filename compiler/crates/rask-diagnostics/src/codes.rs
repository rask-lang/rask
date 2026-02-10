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
