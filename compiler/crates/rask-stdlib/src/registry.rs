// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Implementation method registry for drift detection and layer classification.
//!
//! Lists methods the interpreter actually handles, per type and module.
//! The drift test in rask-interp exercises the interpreter against these
//! lists to catch registered-but-unimplemented methods.
//!
//! Also classifies each type and module by layer — codegen uses this to
//! decide what needs FFI stubs (Runtime) vs what can compile from Rask (Pure).
//!
//! Separate from the spec MethodDefs in types.rs — the spec defines
//! the planned API, this tracks what's implemented today.

/// Where a stdlib type or module lives in the compilation pipeline.
///
/// - `Runtime`: needs OS access — stays in Rust as part of `rask-rt`
/// - `Pure`: no OS access — can be rewritten in Rask once codegen works
/// - `Hybrid`: mix of both (e.g., Duration is pure, Instant needs OS)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibLayer {
    Runtime,
    Pure,
    Hybrid,
}

/// Classify a builtin type by its runtime requirements.
pub fn type_layer(type_name: &str) -> StdlibLayer {
    match type_name {
        "i64" | "i128" | "u128" | "f64" | "bool" | "char" | "string"
        | "Vec" | "Map" | "Pool" | "Handle"
        | "Result" | "Option"
        | "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8"
        | "JsonValue" | "Path" | "Args" | "Duration" => StdlibLayer::Pure,

        "ThreadHandle" | "Sender" | "Receiver" | "Shared"
        | "AtomicBool" | "AtomicI8" | "AtomicU8"
        | "AtomicI16" | "AtomicU16" | "AtomicI32" | "AtomicU32"
        | "AtomicI64" | "AtomicU64" | "AtomicUsize" | "AtomicIsize"
        | "File" | "Metadata"
        | "TcpListener" | "TcpConnection"
        | "Instant" => StdlibLayer::Runtime,

        _ => StdlibLayer::Runtime,
    }
}

/// Classify a stdlib module by its runtime requirements.
pub fn module_layer(module: &str) -> StdlibLayer {
    match module {
        "json" | "math" | "path" => StdlibLayer::Pure,
        "fs" | "io" | "net" | "os" | "cli" => StdlibLayer::Runtime,
        "time" | "random" => StdlibLayer::Hybrid,
        _ => StdlibLayer::Runtime,
    }
}

// ---------------------------------------------------------------------------
// Instance methods by type
// ---------------------------------------------------------------------------

const I64_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem", "neg",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "abs", "min", "max", "to_string", "to_float",
];

const I128_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem", "neg",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "abs", "min", "max", "to_string",
];

const U128_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "min", "max", "to_string",
];

const F64_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "neg",
    "eq", "lt", "le", "gt", "ge",
    "abs", "floor", "ceil", "round", "sqrt",
    "min", "max", "to_string", "to_int", "pow",
];

const BOOL_METHODS: &[&str] = &["eq"];

const CHAR_METHODS: &[&str] = &[
    "is_whitespace", "is_alphabetic", "is_alphanumeric",
    "is_digit", "is_uppercase", "is_lowercase",
    "to_uppercase", "to_lowercase", "eq",
];

const STRING_METHODS: &[&str] = &[
    "len", "is_empty", "clone", "starts_with", "ends_with", "contains",
    "push", "push_str", "trim", "trim_start", "trim_end",
    "to_string", "to_owned", "to_uppercase", "to_lowercase",
    "split", "split_whitespace", "chars", "lines",
    "replace", "substring", "parse_int", "parse",
    "char_at", "byte_at", "parse_float", "index_of",
    "repeat", "reverse", "eq", "ne",
];

const VEC_METHODS: &[&str] = &[
    "push", "pop", "len", "get", "is_empty", "clear",
    "iter", "skip", "take", "first", "last", "contains",
    "reverse", "join", "eq", "ne", "clone", "to_vec",
    "insert", "remove", "collect", "chunks",
    "filter", "map", "flat_map", "fold", "reduce",
    "enumerate", "zip", "limit", "flatten",
    "sort", "sort_by", "any", "all", "find", "position",
    "dedup", "sum", "min", "max",
];

const MAP_METHODS: &[&str] = &[
    "insert", "get", "remove", "contains", "keys", "values",
    "len", "is_empty", "clear", "iter", "clone",
];

const POOL_METHODS: &[&str] = &[
    "insert", "alloc", "get", "get_mut", "remove",
    "len", "is_empty", "contains", "clear",
    "handles", "cursor", "clone",
];

const HANDLE_METHODS: &[&str] = &["eq", "ne"];

const RESULT_METHODS: &[&str] = &[
    "map_err", "map", "ok", "unwrap_or", "is_ok", "is_err", "unwrap",
];

const OPTION_METHODS: &[&str] = &[
    "unwrap_or", "is_some", "is_none", "map", "unwrap",
];

const FILE_METHODS: &[&str] = &[
    "close", "read_all", "read_text", "write", "write_line", "lines",
];

const METADATA_METHODS: &[&str] = &["size", "accessed", "modified"];

const TCP_LISTENER_METHODS: &[&str] = &["accept", "close", "clone"];

const TCP_CONNECTION_METHODS: &[&str] = &[
    "read_all", "write_all", "remote_addr",
    "read_http_request", "write_http_response",
    "close", "clone",
];

const JSON_VALUE_METHODS: &[&str] = &[
    "is_null", "as_bool", "as_number", "as_string", "as_array", "as_object",
];

const DURATION_METHODS: &[&str] = &[
    "as_secs", "as_millis", "as_micros", "as_nanos", "as_secs_f32", "as_secs_f64",
];

const INSTANT_METHODS: &[&str] = &["duration_since", "elapsed"];

const PATH_METHODS: &[&str] = &[
    "parent", "file_name", "extension", "stem", "components",
    "is_absolute", "is_relative", "has_extension",
    "join", "with_extension", "with_file_name", "to_string",
];

const ARGS_METHODS: &[&str] = &[
    "flag", "option", "option_or", "positional", "program",
];

const THREAD_HANDLE_METHODS: &[&str] = &["join", "detach"];
const SENDER_METHODS: &[&str] = &["send"];
const RECEIVER_METHODS: &[&str] = &["recv", "try_recv"];
const SHARED_METHODS: &[&str] = &["read", "write", "clone"];
const SIMD_METHODS: &[&str] = &[
    "splat", "load", "store",
    "add", "sub", "mul", "div", "scale",
    "sum", "product", "min", "max",
    "get", "set",
];

const ATOMIC_BOOL_METHODS: &[&str] = &[
    "new", "default", "load", "store", "swap",
    "compare_exchange", "compare_exchange_weak",
    "fetch_and", "fetch_or", "fetch_xor", "fetch_nand",
    "into_inner",
];
const ATOMIC_INT_METHODS: &[&str] = &[
    "new", "default", "load", "store", "swap",
    "compare_exchange", "compare_exchange_weak",
    "fetch_add", "fetch_sub", "fetch_and", "fetch_or",
    "fetch_xor", "fetch_nand", "fetch_max", "fetch_min",
    "into_inner",
];

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

const FS_METHODS: &[&str] = &[
    "read_file", "read_lines", "write_file", "append_file",
    "exists", "open", "create", "canonicalize", "metadata",
    "remove", "remove_dir", "create_dir", "create_dir_all",
    "rename", "copy",
];

const NET_METHODS: &[&str] = &["tcp_listen", "tcp_connect"];

const JSON_METHODS: &[&str] = &[
    "parse", "stringify", "stringify_pretty",
    "encode", "encode_pretty", "to_value", "decode",
];

const TIME_METHODS: &[&str] = &["sleep"];

const MATH_METHODS: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
    "exp", "ln", "log2", "log10",
    "hypot", "clamp", "to_radians", "to_degrees",
    "is_nan", "is_inf", "is_finite",
];

const RANDOM_METHODS: &[&str] = &["f32", "f64", "i64", "bool", "range"];

const OS_METHODS: &[&str] = &[
    "env", "env_or", "set_env", "remove_env", "vars",
    "args", "exit", "getpid", "platform", "arch",
];

const IO_METHODS: &[&str] = &["read_line"];

const CLI_METHODS: &[&str] = &["args", "parse"];

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// All types with registered instance methods.
pub const REGISTERED_TYPES: &[&str] = &[
    "i64", "i128", "u128", "f64", "bool", "char", "string",
    "Vec", "Map", "Pool", "Handle",
    "Result", "Option",
    "File", "Metadata",
    "TcpListener", "TcpConnection",
    "JsonValue",
    "Duration", "Instant",
    "Path", "Args",
    "ThreadHandle", "Sender", "Receiver", "Shared",
    "AtomicBool", "AtomicI8", "AtomicU8",
    "AtomicI16", "AtomicU16", "AtomicI32", "AtomicU32",
    "AtomicI64", "AtomicU64", "AtomicUsize", "AtomicIsize",
    "f32x4", "f32x8", "f64x2", "f64x4", "i32x4", "i32x8",
];

/// All modules with registered functions.
pub const REGISTERED_MODULES: &[&str] = &[
    "fs", "net", "json", "time", "math", "random", "os", "io", "cli",
];

/// Get implemented method names for a type.
pub fn type_method_names(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "i64" => I64_METHODS,
        "i128" => I128_METHODS,
        "u128" => U128_METHODS,
        "f64" => F64_METHODS,
        "bool" => BOOL_METHODS,
        "char" => CHAR_METHODS,
        "string" => STRING_METHODS,
        "Vec" => VEC_METHODS,
        "Map" => MAP_METHODS,
        "Pool" => POOL_METHODS,
        "Handle" => HANDLE_METHODS,
        "Result" => RESULT_METHODS,
        "Option" => OPTION_METHODS,
        "File" => FILE_METHODS,
        "Metadata" => METADATA_METHODS,
        "TcpListener" => TCP_LISTENER_METHODS,
        "TcpConnection" => TCP_CONNECTION_METHODS,
        "JsonValue" => JSON_VALUE_METHODS,
        "Duration" => DURATION_METHODS,
        "Instant" => INSTANT_METHODS,
        "Path" => PATH_METHODS,
        "Args" => ARGS_METHODS,
        "ThreadHandle" => THREAD_HANDLE_METHODS,
        "Sender" => SENDER_METHODS,
        "Receiver" => RECEIVER_METHODS,
        "Shared" => SHARED_METHODS,
        "AtomicBool" => ATOMIC_BOOL_METHODS,
        "AtomicI8" | "AtomicU8" | "AtomicI16" | "AtomicU16"
        | "AtomicI32" | "AtomicU32" | "AtomicI64" | "AtomicU64"
        | "AtomicUsize" | "AtomicIsize" => ATOMIC_INT_METHODS,
        "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8" => SIMD_METHODS,
        _ => &[],
    }
}

/// Get implemented method names for a module.
pub fn module_method_names(module: &str) -> &'static [&'static str] {
    match module {
        "fs" => FS_METHODS,
        "net" => NET_METHODS,
        "json" => JSON_METHODS,
        "time" => TIME_METHODS,
        "math" => MATH_METHODS,
        "random" => RANDOM_METHODS,
        "os" => OS_METHODS,
        "io" => IO_METHODS,
        "cli" => CLI_METHODS,
        _ => &[],
    }
}

/// Types that exist only for codegen — the interpreter doesn't dispatch them.
/// Drift tests should skip these.
pub fn is_codegen_only_type(type_name: &str) -> bool {
    matches!(type_name,
        "AtomicI8" | "AtomicU8" | "AtomicI16" | "AtomicU16"
        | "AtomicI32" | "AtomicU32" | "AtomicI64" | "AtomicIsize"
        | "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8"
    )
}

/// Methods that exist only for codegen on types the interpreter partially covers.
/// Returns methods to skip for drift testing.
pub fn codegen_only_methods(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "AtomicBool" => &[
            "new", "default", "swap",
            "compare_exchange", "compare_exchange_weak",
            "fetch_and", "fetch_or", "fetch_xor", "fetch_nand",
            "into_inner",
        ],
        "AtomicUsize" | "AtomicU64" => &[
            "new", "default", "swap",
            "compare_exchange", "compare_exchange_weak",
            "fetch_add", "fetch_sub", "fetch_and", "fetch_or",
            "fetch_xor", "fetch_nand", "fetch_max", "fetch_min",
            "into_inner",
        ],
        _ => &[],
    }
}

/// Check if a type has a registered method.
pub fn has_type_method(type_name: &str, method: &str) -> bool {
    type_method_names(type_name).contains(&method)
}

/// Check if a module has a registered function.
pub fn has_module_method(module: &str, method: &str) -> bool {
    module_method_names(module).contains(&method)
}
