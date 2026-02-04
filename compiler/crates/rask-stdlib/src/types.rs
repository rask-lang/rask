//! Type method definitions for the Rask stdlib
//!
//! Defines what methods exist on built-in types like Vec, Map, String.

/// Method signature information
#[derive(Debug, Clone)]
pub struct MethodDef {
    pub name: &'static str,
    pub takes_self: bool,
    pub params: &'static [(&'static str, &'static str)], // (name, type)
    pub ret_ty: &'static str,
}

/// Get methods for Vec<T>
pub fn vec_methods() -> &'static [MethodDef] {
    &[
        // Construction (static)
        MethodDef { name: "new", takes_self: false, params: &[], ret_ty: "Vec<T>" },
        MethodDef { name: "with_capacity", takes_self: false, params: &[("n", "usize")], ret_ty: "Vec<T>" },
        MethodDef { name: "fixed", takes_self: false, params: &[("n", "usize")], ret_ty: "Vec<T>" },

        // Length and capacity
        MethodDef { name: "len", takes_self: true, params: &[], ret_ty: "usize" },
        MethodDef { name: "is_empty", takes_self: true, params: &[], ret_ty: "bool" },
        MethodDef { name: "capacity", takes_self: true, params: &[], ret_ty: "Option<usize>" },
        MethodDef { name: "is_bounded", takes_self: true, params: &[], ret_ty: "bool" },
        MethodDef { name: "remaining", takes_self: true, params: &[], ret_ty: "Option<usize>" },
        MethodDef { name: "allocated", takes_self: true, params: &[], ret_ty: "usize" },

        // Mutation (fallible)
        MethodDef { name: "push", takes_self: true, params: &[("value", "T")], ret_ty: "Result<(), PushError<T>>" },
        MethodDef { name: "push_or_panic", takes_self: true, params: &[("value", "T")], ret_ty: "()" },
        MethodDef { name: "pop", takes_self: true, params: &[], ret_ty: "Option<T>" },
        MethodDef { name: "clear", takes_self: true, params: &[], ret_ty: "()" },
        MethodDef { name: "reserve", takes_self: true, params: &[("additional", "usize")], ret_ty: "Result<(), AllocError>" },

        // Access
        MethodDef { name: "get", takes_self: true, params: &[("index", "usize")], ret_ty: "Option<T>" },
        MethodDef { name: "get_clone", takes_self: true, params: &[("index", "usize")], ret_ty: "Option<T>" },
        MethodDef { name: "first", takes_self: true, params: &[], ret_ty: "Option<T>" },
        MethodDef { name: "last", takes_self: true, params: &[], ret_ty: "Option<T>" },

        // Closure access
        MethodDef { name: "read", takes_self: true, params: &[("index", "usize"), ("f", "func(T) -> R")], ret_ty: "Option<R>" },
        MethodDef { name: "modify", takes_self: true, params: &[("index", "usize"), ("f", "func(T) -> R")], ret_ty: "Option<R>" },

        // Multi-element
        MethodDef { name: "swap", takes_self: true, params: &[("i", "usize"), ("j", "usize")], ret_ty: "()" },

        // Iteration
        MethodDef { name: "iter", takes_self: true, params: &[], ret_ty: "Iterator<T>" },
        MethodDef { name: "take_all", takes_self: true, params: &[], ret_ty: "Iterator<T>" },

        // Filtering
        MethodDef { name: "retain", takes_self: true, params: &[("f", "func(T) -> bool")], ret_ty: "()" },
        MethodDef { name: "remove_where", takes_self: true, params: &[("f", "func(T) -> bool")], ret_ty: "usize" },
        MethodDef { name: "drain_where", takes_self: true, params: &[("f", "func(T) -> bool")], ret_ty: "Vec<T>" },

        // Shrinking
        MethodDef { name: "shrink_to_fit", takes_self: true, params: &[], ret_ty: "()" },
        MethodDef { name: "shrink_to", takes_self: true, params: &[("min_capacity", "usize")], ret_ty: "()" },

        // In-place construction
        MethodDef { name: "push_with", takes_self: true, params: &[("f", "func(T)")], ret_ty: "Result<usize, AllocError>" },

        // FFI (unsafe)
        MethodDef { name: "as_ptr", takes_self: true, params: &[], ret_ty: "*const T" },
        MethodDef { name: "as_mut_ptr", takes_self: true, params: &[], ret_ty: "*mut T" },

        // Comptime
        MethodDef { name: "freeze", takes_self: true, params: &[], ret_ty: "[T; N]" },
    ]
}

/// Get methods for Map<K, V>
pub fn map_methods() -> &'static [MethodDef] {
    &[
        // Construction (static)
        MethodDef { name: "new", takes_self: false, params: &[], ret_ty: "Map<K, V>" },
        MethodDef { name: "with_capacity", takes_self: false, params: &[("n", "usize")], ret_ty: "Map<K, V>" },

        // Length and capacity
        MethodDef { name: "len", takes_self: true, params: &[], ret_ty: "usize" },
        MethodDef { name: "is_empty", takes_self: true, params: &[], ret_ty: "bool" },
        MethodDef { name: "capacity", takes_self: true, params: &[], ret_ty: "Option<usize>" },
        MethodDef { name: "is_bounded", takes_self: true, params: &[], ret_ty: "bool" },

        // Mutation (fallible)
        MethodDef { name: "insert", takes_self: true, params: &[("key", "K"), ("value", "V")], ret_ty: "Result<Option<V>, InsertError<V>>" },
        MethodDef { name: "remove", takes_self: true, params: &[("key", "K")], ret_ty: "Option<V>" },
        MethodDef { name: "clear", takes_self: true, params: &[], ret_ty: "()" },

        // Access
        MethodDef { name: "get", takes_self: true, params: &[("key", "K")], ret_ty: "Option<V>" },
        MethodDef { name: "get_clone", takes_self: true, params: &[("key", "K")], ret_ty: "Option<V>" },
        MethodDef { name: "contains_key", takes_self: true, params: &[("key", "K")], ret_ty: "bool" },

        // Closure access
        MethodDef { name: "read", takes_self: true, params: &[("key", "K"), ("f", "func(V) -> R")], ret_ty: "Option<R>" },
        MethodDef { name: "modify", takes_self: true, params: &[("key", "K"), ("f", "func(V) -> R")], ret_ty: "Option<R>" },

        // Entry API
        MethodDef { name: "ensure", takes_self: true, params: &[("key", "K"), ("f", "func() -> V")], ret_ty: "Result<(), InsertError>" },
        MethodDef { name: "ensure_modify", takes_self: true, params: &[("key", "K"), ("default", "func() -> V"), ("f", "func(V) -> R")], ret_ty: "Result<R, InsertError>" },

        // Iteration
        MethodDef { name: "iter", takes_self: true, params: &[], ret_ty: "Iterator<(K, V)>" },
        MethodDef { name: "keys", takes_self: true, params: &[], ret_ty: "Iterator<K>" },
        MethodDef { name: "values", takes_self: true, params: &[], ret_ty: "Iterator<V>" },

        // Comptime
        MethodDef { name: "freeze", takes_self: true, params: &[], ret_ty: "Map<K, V>" },
    ]
}

/// Get methods for String
pub fn string_methods() -> &'static [MethodDef] {
    &[
        // Construction (static)
        MethodDef { name: "new", takes_self: false, params: &[], ret_ty: "string" },
        MethodDef { name: "with_capacity", takes_self: false, params: &[("n", "usize")], ret_ty: "string" },

        // Length
        MethodDef { name: "len", takes_self: true, params: &[], ret_ty: "usize" },
        MethodDef { name: "is_empty", takes_self: true, params: &[], ret_ty: "bool" },

        // Content checks
        MethodDef { name: "contains", takes_self: true, params: &[("s", "string")], ret_ty: "bool" },
        MethodDef { name: "starts_with", takes_self: true, params: &[("s", "string")], ret_ty: "bool" },
        MethodDef { name: "ends_with", takes_self: true, params: &[("s", "string")], ret_ty: "bool" },

        // Search
        MethodDef { name: "find", takes_self: true, params: &[("s", "string")], ret_ty: "Option<usize>" },
        MethodDef { name: "rfind", takes_self: true, params: &[("s", "string")], ret_ty: "Option<usize>" },

        // Transformation
        MethodDef { name: "to_uppercase", takes_self: true, params: &[], ret_ty: "string" },
        MethodDef { name: "to_lowercase", takes_self: true, params: &[], ret_ty: "string" },
        MethodDef { name: "trim", takes_self: true, params: &[], ret_ty: "string" },
        MethodDef { name: "trim_start", takes_self: true, params: &[], ret_ty: "string" },
        MethodDef { name: "trim_end", takes_self: true, params: &[], ret_ty: "string" },

        // Mutation
        MethodDef { name: "push", takes_self: true, params: &[("c", "char")], ret_ty: "()" },
        MethodDef { name: "push_str", takes_self: true, params: &[("s", "string")], ret_ty: "()" },
        MethodDef { name: "clear", takes_self: true, params: &[], ret_ty: "()" },

        // Splitting
        MethodDef { name: "split", takes_self: true, params: &[("sep", "string")], ret_ty: "Iterator<string>" },
        MethodDef { name: "lines", takes_self: true, params: &[], ret_ty: "Iterator<string>" },
        MethodDef { name: "chars", takes_self: true, params: &[], ret_ty: "Iterator<char>" },
        MethodDef { name: "bytes", takes_self: true, params: &[], ret_ty: "Iterator<u8>" },

        // Replacement
        MethodDef { name: "replace", takes_self: true, params: &[("from", "string"), ("to", "string")], ret_ty: "string" },
        MethodDef { name: "replacen", takes_self: true, params: &[("from", "string"), ("to", "string"), ("n", "usize")], ret_ty: "string" },

        // Parsing
        MethodDef { name: "parse", takes_self: true, params: &[], ret_ty: "Result<T, ParseError>" },

        // Iteration
        MethodDef { name: "iter", takes_self: true, params: &[], ret_ty: "Iterator<char>" },
    ]
}

/// Get methods for Pool<T>
pub fn pool_methods() -> &'static [MethodDef] {
    &[
        // Construction
        MethodDef { name: "new", takes_self: false, params: &[], ret_ty: "Pool<T>" },
        MethodDef { name: "with_capacity", takes_self: false, params: &[("n", "usize")], ret_ty: "Pool<T>" },

        // Length
        MethodDef { name: "len", takes_self: true, params: &[], ret_ty: "usize" },
        MethodDef { name: "is_empty", takes_self: true, params: &[], ret_ty: "bool" },

        // Insertion
        MethodDef { name: "insert", takes_self: true, params: &[("value", "T")], ret_ty: "Result<Handle<T>, AllocError>" },

        // Access
        MethodDef { name: "get", takes_self: true, params: &[("handle", "Handle<T>")], ret_ty: "Option<T>" },
        MethodDef { name: "get_clone", takes_self: true, params: &[("handle", "Handle<T>")], ret_ty: "Option<T>" },
        MethodDef { name: "read", takes_self: true, params: &[("handle", "Handle<T>"), ("f", "func(T) -> R")], ret_ty: "Option<R>" },
        MethodDef { name: "modify", takes_self: true, params: &[("handle", "Handle<T>"), ("f", "func(T) -> R")], ret_ty: "Option<R>" },
        MethodDef { name: "contains", takes_self: true, params: &[("handle", "Handle<T>")], ret_ty: "bool" },

        // Removal
        MethodDef { name: "remove", takes_self: true, params: &[("handle", "Handle<T>")], ret_ty: "Option<T>" },
        MethodDef { name: "clear", takes_self: true, params: &[], ret_ty: "()" },

        // Iteration
        MethodDef { name: "iter", takes_self: true, params: &[], ret_ty: "Iterator<(Handle<T>, T)>" },
        MethodDef { name: "handles", takes_self: true, params: &[], ret_ty: "Iterator<Handle<T>>" },
        MethodDef { name: "values", takes_self: true, params: &[], ret_ty: "Iterator<T>" },
    ]
}

/// Get methods for Option<T>
pub fn option_methods() -> &'static [MethodDef] {
    &[
        MethodDef { name: "is_some", takes_self: true, params: &[], ret_ty: "bool" },
        MethodDef { name: "is_none", takes_self: true, params: &[], ret_ty: "bool" },
        MethodDef { name: "unwrap", takes_self: true, params: &[], ret_ty: "T" },
        MethodDef { name: "unwrap_or", takes_self: true, params: &[("default", "T")], ret_ty: "T" },
        MethodDef { name: "unwrap_or_else", takes_self: true, params: &[("f", "func() -> T")], ret_ty: "T" },
        MethodDef { name: "map", takes_self: true, params: &[("f", "func(T) -> U")], ret_ty: "Option<U>" },
        MethodDef { name: "and_then", takes_self: true, params: &[("f", "func(T) -> Option<U>")], ret_ty: "Option<U>" },
        MethodDef { name: "or", takes_self: true, params: &[("other", "Option<T>")], ret_ty: "Option<T>" },
        MethodDef { name: "or_else", takes_self: true, params: &[("f", "func() -> Option<T>")], ret_ty: "Option<T>" },
        MethodDef { name: "ok_or", takes_self: true, params: &[("err", "E")], ret_ty: "Result<T, E>" },
    ]
}

/// Get methods for Result<T, E>
pub fn result_methods() -> &'static [MethodDef] {
    &[
        MethodDef { name: "is_ok", takes_self: true, params: &[], ret_ty: "bool" },
        MethodDef { name: "is_err", takes_self: true, params: &[], ret_ty: "bool" },
        MethodDef { name: "unwrap", takes_self: true, params: &[], ret_ty: "T" },
        MethodDef { name: "unwrap_err", takes_self: true, params: &[], ret_ty: "E" },
        MethodDef { name: "unwrap_or", takes_self: true, params: &[("default", "T")], ret_ty: "T" },
        MethodDef { name: "unwrap_or_else", takes_self: true, params: &[("f", "func(E) -> T")], ret_ty: "T" },
        MethodDef { name: "map", takes_self: true, params: &[("f", "func(T) -> U")], ret_ty: "Result<U, E>" },
        MethodDef { name: "map_err", takes_self: true, params: &[("f", "func(E) -> F")], ret_ty: "Result<T, F>" },
        MethodDef { name: "and_then", takes_self: true, params: &[("f", "func(T) -> Result<U, E>")], ret_ty: "Result<U, E>" },
        MethodDef { name: "or_else", takes_self: true, params: &[("f", "func(E) -> Result<T, F>")], ret_ty: "Result<T, F>" },
        MethodDef { name: "ok", takes_self: true, params: &[], ret_ty: "Option<T>" },
        MethodDef { name: "err", takes_self: true, params: &[], ret_ty: "Option<E>" },
    ]
}

/// Look up a method by type name and method name
pub fn lookup_method(type_name: &str, method_name: &str) -> Option<&'static MethodDef> {
    let methods: &[MethodDef] = match type_name {
        "Vec" => vec_methods(),
        "Map" => map_methods(),
        "String" | "string" => string_methods(),
        "Pool" => pool_methods(),
        "Option" => option_methods(),
        "Result" => result_methods(),
        _ => return None,
    };

    methods.iter().find(|m| m.name == method_name)
}

/// Check if a method exists on a type
pub fn has_method(type_name: &str, method_name: &str) -> bool {
    lookup_method(type_name, method_name).is_some()
}
