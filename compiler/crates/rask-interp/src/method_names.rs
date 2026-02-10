// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Interpreter method name constants.
//!
//! Every method handled by the interpreter's dispatch code is listed here.
//! The drift test in tests/drift_test.rs cross-checks these against rask-stdlib
//! to catch mismatches between type checker and interpreter.

// ---------------------------------------------------------------------------
// Instance methods on builtin types
// ---------------------------------------------------------------------------

/// Methods on string values (builtins/strings.rs)
pub const STRING_METHODS: &[&str] = &[
    "len", "is_empty", "clone", "starts_with", "ends_with", "contains",
    "push", "push_str", "trim", "trim_start", "trim_end",
    "to_string", "to_owned", "to_uppercase", "to_lowercase",
    "split", "split_whitespace", "chars", "lines",
    "replace", "substring", "parse_int", "parse",
    "char_at", "byte_at", "parse_float", "index_of",
    "repeat", "reverse", "eq", "ne",
];

/// Methods on Vec values (builtins/collections.rs)
pub const VEC_METHODS: &[&str] = &[
    "push", "pop", "len", "get", "is_empty", "clear",
    "iter", "skip", "take", "first", "last", "contains",
    "reverse", "join", "eq", "ne", "clone", "to_vec",
    "insert", "remove", "collect", "chunks",
    "filter", "map", "flat_map", "fold", "reduce",
    "enumerate", "zip", "limit", "flatten",
    "sort", "sort_by", "any", "all", "find", "position",
    "dedup", "sum", "min", "max",
];

/// Methods on Map values (builtins/collections.rs)
pub const MAP_METHODS: &[&str] = &[
    "insert", "get", "remove", "contains", "keys", "values",
    "len", "is_empty", "clear", "iter", "clone",
];

/// Methods on Pool values (builtins/collections.rs)
pub const POOL_METHODS: &[&str] = &[
    "insert", "alloc", "get", "get_mut", "remove",
    "len", "is_empty", "contains", "clear",
    "handles", "cursor", "clone",
];

/// Methods on Handle values (builtins/collections.rs)
pub const HANDLE_METHODS: &[&str] = &["eq", "ne"];

/// Methods on i64 values (builtins/primitives.rs)
pub const INT_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem", "neg",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "abs", "min", "max", "to_string", "to_float",
    "add_assign", "sub_assign", "mul_assign", "div_assign", "rem_assign",
    "bitand_assign", "bitor_assign", "bitxor_assign", "shl_assign", "shr_assign",
];

/// Methods on i128 values (builtins/primitives.rs)
pub const INT128_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem", "neg",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "abs", "min", "max", "to_string",
];

/// Methods on u128 values (builtins/primitives.rs)
pub const UINT128_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "rem",
    "eq", "lt", "le", "gt", "ge",
    "bit_and", "bit_or", "bit_xor", "shl", "shr", "bit_not",
    "min", "max", "to_string",
];

/// Methods on f64 values (builtins/primitives.rs)
pub const FLOAT_METHODS: &[&str] = &[
    "add", "sub", "mul", "div", "neg",
    "eq", "lt", "le", "gt", "ge",
    "abs", "floor", "ceil", "round", "sqrt",
    "min", "max", "to_string", "to_int", "pow",
];

/// Methods on bool values (builtins/primitives.rs)
pub const BOOL_METHODS: &[&str] = &["eq"];

/// Methods on char values (builtins/primitives.rs)
pub const CHAR_METHODS: &[&str] = &[
    "is_whitespace", "is_alphabetic", "is_alphanumeric",
    "is_digit", "is_uppercase", "is_lowercase",
    "to_uppercase", "to_lowercase", "eq",
];

/// Methods on Result values (builtins/enums.rs)
pub const RESULT_METHODS: &[&str] = &[
    "map_err", "map", "ok", "unwrap_or", "is_ok", "is_err", "unwrap",
];

/// Methods on Option values (builtins/enums.rs)
pub const OPTION_METHODS: &[&str] = &[
    "unwrap_or", "is_some", "is_none", "map", "unwrap",
];

/// Methods on ThreadHandle values (builtins/threading.rs)
pub const THREAD_HANDLE_METHODS: &[&str] = &["join", "detach"];

/// Methods on Sender values (builtins/threading.rs)
pub const SENDER_METHODS: &[&str] = &["send"];

/// Methods on Receiver values (builtins/threading.rs)
pub const RECEIVER_METHODS: &[&str] = &["recv", "try_recv"];

/// Methods on Shared values (builtins/shared.rs)
pub const SHARED_METHODS: &[&str] = &["read", "write", "clone"];

/// Methods on AtomicBool values (builtins/collections.rs)
pub const ATOMIC_BOOL_METHODS: &[&str] = &["load", "store"];

/// Methods on AtomicUsize values (builtins/collections.rs)
pub const ATOMIC_USIZE_METHODS: &[&str] = &["load", "store"];

/// Methods on AtomicU64 values (builtins/collections.rs)
pub const ATOMIC_U64_METHODS: &[&str] = &["load", "store"];

/// Methods on File instances (stdlib/fs.rs)
pub const FILE_METHODS: &[&str] = &[
    "close", "read_all", "read_text", "write", "write_line", "lines",
];

/// Methods on Metadata instances (stdlib/fs.rs)
pub const METADATA_METHODS: &[&str] = &["size", "accessed", "modified"];

/// Methods on TcpListener instances (stdlib/net.rs)
pub const TCP_LISTENER_METHODS: &[&str] = &["accept", "close", "clone"];

/// Methods on TcpConnection instances (stdlib/net.rs)
pub const TCP_CONNECTION_METHODS: &[&str] = &[
    "read_all", "write_all", "remote_addr",
    "read_http_request", "write_http_response",
    "close", "clone",
];

/// Methods on JsonValue instances (stdlib/json.rs)
pub const JSON_VALUE_METHODS: &[&str] = &[
    "is_null", "as_bool", "as_number", "as_string", "as_array", "as_object",
];

/// Methods on Duration instances (stdlib/time.rs)
pub const DURATION_METHODS: &[&str] = &[
    "as_secs", "as_millis", "as_micros", "as_nanos", "as_secs_f32", "as_secs_f64",
];

/// Methods on Instant instances (stdlib/time.rs)
pub const INSTANT_METHODS: &[&str] = &["duration_since", "elapsed"];

/// Methods on Path instances (stdlib/path.rs)
pub const PATH_METHODS: &[&str] = &[
    "parent", "file_name", "extension", "stem", "components",
    "is_absolute", "is_relative", "has_extension",
    "join", "with_extension", "with_file_name", "to_string",
];

/// Methods on Args instances (stdlib/cli.rs)
pub const ARGS_METHODS: &[&str] = &[
    "flag", "option", "option_or", "positional", "program",
];

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

/// fs module functions (stdlib/fs.rs)
pub const FS_MODULE_METHODS: &[&str] = &[
    "read_file", "read_lines", "write_file", "append_file",
    "exists", "open", "create", "canonicalize", "metadata",
    "remove", "remove_dir", "create_dir", "create_dir_all",
    "rename", "copy",
];

/// net module functions (stdlib/net.rs)
pub const NET_MODULE_METHODS: &[&str] = &["tcp_listen", "tcp_connect"];

/// json module functions (stdlib/json.rs)
pub const JSON_MODULE_METHODS: &[&str] = &[
    "parse", "stringify", "stringify_pretty",
    "encode", "encode_pretty", "to_value", "decode",
];

/// time module functions (stdlib/time.rs)
pub const TIME_MODULE_METHODS: &[&str] = &["sleep"];

/// math module functions (stdlib/math.rs)
pub const MATH_MODULE_METHODS: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
    "exp", "ln", "log2", "log10",
    "hypot", "clamp", "to_radians", "to_degrees",
    "is_nan", "is_inf", "is_finite",
];

/// random module functions (stdlib/random.rs)
pub const RANDOM_MODULE_METHODS: &[&str] = &["f32", "f64", "i64", "bool", "range"];

/// os module functions (stdlib/os.rs)
pub const OS_MODULE_METHODS: &[&str] = &[
    "env", "env_or", "set_env", "remove_env", "vars",
    "args", "exit", "getpid", "platform", "arch",
];

/// io module functions (stdlib/io.rs)
pub const IO_MODULE_METHODS: &[&str] = &["read_line"];

/// cli module functions (stdlib/cli.rs)
pub const CLI_MODULE_METHODS: &[&str] = &["args", "parse"];
