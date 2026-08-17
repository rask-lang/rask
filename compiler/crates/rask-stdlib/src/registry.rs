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
/// - `Runtime`: needs OS access — implemented in the C runtime
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
        "i8" | "i16" | "i32" | "i64" | "i128"
        | "u8" | "u16" | "u32" | "u64" | "u128"
        | "f64" | "bool" | "char" | "string"
        | "Vec" | "Map" | "Pool" | "Handle" | "Store" | "Link"
        | "Result" | "Option"
        | "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8"
        | "JsonValue" | "Path" | "Args" | "Duration" => StdlibLayer::Pure,

        "ThreadHandle" | "Sender" | "Receiver" | "Shared" | "Mutex" | "Cell"
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

const SIGNED_INT_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem", "neg",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "abs", "to_string", "to_float",
];

const UNSIGNED_INT_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "to_string", "to_float",
];

const I64_METHODS: &[&str] = SIGNED_INT_METHODS;

const I128_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem", "neg",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "abs", "to_string",
];

const U128_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "to_string",
];

/// Float methods come from `float_methods::FLOAT_METHODS` — the checker,
/// interpreter and codegen read that same table, so this list can't drift
/// from what f64 actually answers to.
fn f64_methods() -> &'static [&'static str] {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(crate::float_methods::method_names).as_slice()
}

const BOOL_METHODS: &[&str] = &["eq", "lt", "le", "gt", "ge", "compare", "to_string"];

const CHAR_METHODS: &[&str] = &[
    "is_whitespace", "is_ascii", "is_alphabetic", "is_numeric",
    "is_alphanumeric", "is_digit", "is_uppercase", "is_lowercase",
    "to_uppercase", "to_lowercase", "len_utf8",
    "to_string", "eq", "lt", "le", "gt", "ge", "compare",
];

const STRING_METHODS: &[&str] = &[
    "len", "is_empty", "clone", "starts_with", "ends_with", "contains",
    "push", "push_str", "trim", "trim_start", "trim_end", "trim_indices",
    // No `to_owned`: it's a Rust name with no entry in std.strings and no
    // signature anywhere, so it resolved as a known method whose return type
    // stayed open — MIR gave the temp `i64`, codegen had no string slot to
    // copy into, and `part.to_owned()` segfaulted. The spec's two storable
    // conversions are `.to_string()` (copies) and `.view()` (zero-copy).
    "to_string", "to_uppercase", "to_lowercase",
    "split", "split_whitespace", "chars", "char_indices", "bytes", "lines",
    "replace", "substring", "parse_int", "parse",
    "char_at", "byte_at", "parse_float", "index_of", "last_index_of",
    "repeat", "reverse", "eq", "ne",
    "char_count", "is_ascii", "replacen",
];

const VEC_METHODS: &[&str] = &[
    "push", "pop", "len", "get", "is_empty", "clear",
    "iter", "skip", "take", "first", "last", "contains",
    "reverse", "swap", "join", "eq", "ne", "clone", "to_vec",
    "insert", "remove", "collect", "chunks",
    "filter", "map", "flat_map", "fold", "reduce",
    "enumerate", "zip", "limit", "flatten",
    "sort", "sort_by", "any", "all", "find", "position",
    "remove_adjacent_duplicates", "sum", "min", "max", "count", "take_all",
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

/// `Store<T>` — structural ops only. There is no `get`: a link is followed by
/// field access, not redeemed at the container (analysis.fourth-option).
const STORE_METHODS: &[&str] = &[
    "insert", "delete", "len", "is_empty", "contains", "nodes", "links", "clear",
];

const LINK_METHODS: &[&str] = &["eq", "ne"];

const RESULT_METHODS: &[&str] = &[
    "map_err", "map", "ok", "unwrap_or", "is_ok", "is_err", "unwrap",
];

const OPTION_METHODS: &[&str] = &[
    "unwrap_or", "is_some", "is_none", "map", "unwrap",
];

const FILE_METHODS: &[&str] = &[
    "close", "read_bytes", "read_text", "write", "write_bytes", "write_text", "write_line",
];

const METADATA_METHODS: &[&str] = &["size", "accessed", "modified"];

const TCP_LISTENER_METHODS: &[&str] = &["accept", "local_addr", "close", "clone"];

const TCP_CONNECTION_METHODS: &[&str] = &[
    "read_bytes", "write_bytes", "read_text", "write_text", "remote_addr",
    "read_http_request", "write_http_response",
    "close", "clone",
];

const JSON_VALUE_METHODS: &[&str] = &[
    "is_null", "as_bool", "as_number", "as_string", "as_array", "as_object",
];

const DURATION_METHODS: &[&str] = &[
    "as_seconds", "as_millis", "as_micros", "as_nanos", "as_seconds_f32", "as_seconds_f64",
];

const INSTANT_METHODS: &[&str] = &["elapsed"];

const PATH_METHODS: &[&str] = &[
    "parent", "file_name", "extension", "stem", "components",
    "is_absolute", "is_relative", "has_extension",
    "div", "with_extension", "with_file_name", "as_string",
];

const ARGS_METHODS: &[&str] = &[
    "flag", "option", "option_or", "positional", "program",
];

const THREAD_HANDLE_METHODS: &[&str] = &["join", "detach"];
const TASK_HANDLE_METHODS: &[&str] = &["join", "detach", "cancel"];
const SENDER_METHODS: &[&str] = &["send", "try_send", "close"];
const RECEIVER_METHODS: &[&str] = &["receive", "try_receive", "close"];
const SHARED_METHODS: &[&str] = &["read", "write", "try_read", "try_write", "clone"];
const MUTEX_METHODS: &[&str] = &["lock", "try_lock", "clone"];
const CELL_METHODS: &[&str] = &["get", "set", "replace", "into_inner"];
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
    "read_text", "read_bytes", "read_lines", "write_text", "write_bytes",
    "append_text", "exists", "open", "create", "absolute_path", "metadata",
    "remove_file", "remove_dir", "create_dir", "create_dir_all",
    "rename", "copy", "current_dir", "home_dir",
];

const NET_METHODS: &[&str] = &["tcp_listen", "tcp_connect"];

// No "stringify"/"stringify_pretty": std.json has one verb pair and no
// parse/stringify family, so they were never declared in stdlib/json.rk and
// nothing could call them. `parse` is the untyped half of `decode`, written in
// Rask.
const JSON_METHODS: &[&str] = &[
    "parse", "encode", "encode_pretty", "to_value", "decode",
];

const TIME_METHODS: &[&str] = &["sleep"];

const MATH_METHODS: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
    "exp", "ln", "log2", "log10",
    "hypot", "to_radians", "to_degrees",
];

const RANDOM_METHODS: &[&str] = &["f32", "f64", "i64", "bool", "range"];

const OS_METHODS: &[&str] = &[
    "env", "env_or", "set_env", "remove_env", "env_vars",
    "args", "exit", "pid", "platform", "arch",
];

const IO_METHODS: &[&str] = &["read_line"];

const CLI_METHODS: &[&str] = &["args", "parse"];

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// All types with registered instance methods.
pub const REGISTERED_TYPES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128",
    "u8", "u16", "u32", "u64", "u128",
    "f64", "bool", "char", "string",
    "Vec", "Map", "Pool", "Handle", "Store", "Link",
    "Result", "Option",
    "File", "Metadata",
    "TcpListener", "TcpConnection",
    "JsonValue",
    "Duration", "Instant",
    "Path", "Args",
    "ThreadHandle", "TaskHandle", "Sender", "Receiver", "Shared", "Mutex", "Cell",
    "AtomicBool", "AtomicI8", "AtomicU8",
    "AtomicI16", "AtomicU16", "AtomicI32", "AtomicU32",
    "AtomicI64", "AtomicU64", "AtomicUsize", "AtomicIsize",
    "f32x4", "f32x8", "f64x2", "f64x4", "i32x4", "i32x8",
];

/// All modules with registered functions.
pub const REGISTERED_MODULES: &[&str] = &[
    "fs", "net", "json", "time", "math", "random", "os", "io", "cli",
];

/// Types whose methods are written in Rask, in `stdlib/*.rk`, rather than
/// natively in each backend.
///
/// Both backends run that source — native through `compilable_decls`, the
/// interpreter through the same — so there is one implementation and it can't
/// disagree with itself. A Rust implementation in `rask-interp/src/stdlib/` for
/// one of these would be a *second* one, which is what the drift test checks.
///
/// `Path` was the first: 46 lines of declarations, 192 lines of C and 184 lines
/// of Rust, for pure string manipulation that the two backends got different
/// answers from (#688).
pub const RASK_IMPLEMENTED_TYPES: &[&str] = &["Path"];

/// True when this type's methods live in Rask rather than in the backends.
pub fn is_rask_implemented(type_name: &str) -> bool {
    RASK_IMPLEMENTED_TYPES.contains(&type_name)
}

/// Get implemented method names for a type.
pub fn type_method_names(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "i8" | "i16" | "i32" | "i64" => SIGNED_INT_METHODS,
        "u8" | "u16" | "u32" | "u64" => UNSIGNED_INT_METHODS,
        "i128" => I128_METHODS,
        "u128" => U128_METHODS,
        "f64" => f64_methods(),
        "bool" => BOOL_METHODS,
        "char" => CHAR_METHODS,
        "string" => STRING_METHODS,
        "Vec" => VEC_METHODS,
        "Map" => MAP_METHODS,
        "Pool" => POOL_METHODS,
        "Handle" => HANDLE_METHODS,
        "Store" => STORE_METHODS,
        "Link" => LINK_METHODS,
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
        "TaskHandle" => TASK_HANDLE_METHODS,
        "Sender" => SENDER_METHODS,
        "Receiver" => RECEIVER_METHODS,
        "Shared" => SHARED_METHODS,
        "Mutex" => MUTEX_METHODS,
        "Cell" => CELL_METHODS,
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
