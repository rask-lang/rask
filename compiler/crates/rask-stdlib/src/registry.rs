// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Implementation method registry for drift detection.
//!
//! Lists methods the interpreter actually handles, per type and module.
//! The drift test in rask-interp exercises the interpreter against these
//! lists to catch registered-but-unimplemented methods.
//!
//! Separate from the spec MethodDefs in types.rs — the spec defines
//! the planned API, this tracks what's implemented today.

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
const ATOMIC_BOOL_METHODS: &[&str] = &["load", "store"];
const ATOMIC_USIZE_METHODS: &[&str] = &["load", "store"];
const ATOMIC_U64_METHODS: &[&str] = &["load", "store"];

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
    "AtomicBool", "AtomicUsize", "AtomicU64",
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
        "AtomicUsize" => ATOMIC_USIZE_METHODS,
        "AtomicU64" => ATOMIC_U64_METHODS,
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

/// Check if a type has a registered method.
pub fn has_type_method(type_name: &str, method: &str) -> bool {
    type_method_names(type_name).contains(&method)
}

/// Check if a module has a registered function.
pub fn has_module_method(module: &str, method: &str) -> bool {
    module_method_names(module).contains(&method)
}
