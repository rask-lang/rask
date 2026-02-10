// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Drift detection test: cross-checks interpreter method lists against rask-stdlib.
//!
//! If you add a method to the interpreter, add it to both method_names.rs and
//! rask-stdlib/src/types.rs. If you add a method to rask-stdlib that the
//! interpreter should handle, implement it and add it to method_names.rs.
//! This test catches either direction of drift.

use rask_interp::method_names;

/// Check that every method the interpreter handles exists in the rask-stdlib registry.
fn check_interp_covered_by_registry(type_name: &str, interp_methods: &[&str]) {
    for &m in interp_methods {
        assert!(
            rask_stdlib::has_method(type_name, m),
            "Interpreter has {}.{} but rask-stdlib doesn't — add it to types.rs",
            type_name, m,
        );
    }
}

/// Check that every method in rask-stdlib is implemented in the interpreter.
/// Only checks types where we expect full coverage (not spec-only methods).
fn check_registry_covered_by_interp(type_name: &str, interp_methods: &[&str]) {
    for def in rask_stdlib::type_methods(type_name) {
        assert!(
            interp_methods.contains(&def.name),
            "rask-stdlib has {}.{} but interpreter doesn't — implement it or remove from registry",
            type_name, def.name,
        );
    }
}

/// Bidirectional check: interpreter ↔ registry must match exactly.
fn check_sync(type_name: &str, interp_methods: &[&str]) {
    check_interp_covered_by_registry(type_name, interp_methods);
    check_registry_covered_by_interp(type_name, interp_methods);
}

/// Check interpreter → registry for module methods.
fn check_module_interp_covered(module: &str, interp_methods: &[&str]) {
    for &m in interp_methods {
        assert!(
            rask_stdlib::has_module_method(module, m),
            "Interpreter has {}.{} but rask-stdlib doesn't — add it to types.rs",
            module, m,
        );
    }
}

/// Check registry → interpreter for module methods.
fn check_module_registry_covered(module: &str, interp_methods: &[&str]) {
    for def in rask_stdlib::module_methods(module) {
        // Skip dotted names like "Duration.seconds" — those are type constructors,
        // not direct module methods in the interpreter dispatch
        if def.name.contains('.') {
            continue;
        }
        assert!(
            interp_methods.contains(&def.name),
            "rask-stdlib has {}.{} but interpreter doesn't — implement it or remove from registry",
            module, def.name,
        );
    }
}

/// Bidirectional check for module methods.
fn check_module_sync(module: &str, interp_methods: &[&str]) {
    check_module_interp_covered(module, interp_methods);
    check_module_registry_covered(module, interp_methods);
}

// ---------------------------------------------------------------------------
// Primitive types — full bidirectional sync
// ---------------------------------------------------------------------------

#[test]
fn int_methods_sync() {
    check_sync("i64", method_names::INT_METHODS);
}

#[test]
fn int128_methods_sync() {
    check_sync("i128", method_names::INT128_METHODS);
}

#[test]
fn uint128_methods_sync() {
    check_sync("u128", method_names::UINT128_METHODS);
}

#[test]
fn float_methods_sync() {
    check_sync("f64", method_names::FLOAT_METHODS);
}

#[test]
fn bool_methods_sync() {
    check_sync("bool", method_names::BOOL_METHODS);
}

#[test]
fn char_methods_sync() {
    check_sync("char", method_names::CHAR_METHODS);
}

// ---------------------------------------------------------------------------
// Collection types — interpreter methods must be in registry, but registry
// may have spec-only methods not yet implemented
// ---------------------------------------------------------------------------

#[test]
fn string_methods_interp_covered() {
    check_interp_covered_by_registry("string", method_names::STRING_METHODS);
}

#[test]
fn vec_methods_interp_covered() {
    check_interp_covered_by_registry("Vec", method_names::VEC_METHODS);
}

#[test]
fn map_methods_interp_covered() {
    check_interp_covered_by_registry("Map", method_names::MAP_METHODS);
}

#[test]
fn pool_methods_interp_covered() {
    check_interp_covered_by_registry("Pool", method_names::POOL_METHODS);
}

#[test]
fn handle_methods_sync() {
    check_sync("Handle", method_names::HANDLE_METHODS);
}

// ---------------------------------------------------------------------------
// Enum types — interpreter methods must be in registry, registry may have
// spec-only methods
// ---------------------------------------------------------------------------

#[test]
fn result_methods_interp_covered() {
    check_interp_covered_by_registry("Result", method_names::RESULT_METHODS);
}

#[test]
fn option_methods_interp_covered() {
    check_interp_covered_by_registry("Option", method_names::OPTION_METHODS);
}

// ---------------------------------------------------------------------------
// I/O types — full sync
// ---------------------------------------------------------------------------

#[test]
fn file_methods_sync() {
    check_sync("File", method_names::FILE_METHODS);
}

#[test]
fn metadata_methods_sync() {
    check_sync("Metadata", method_names::METADATA_METHODS);
}

#[test]
fn tcp_listener_methods_sync() {
    check_sync("TcpListener", method_names::TCP_LISTENER_METHODS);
}

#[test]
fn tcp_connection_methods_sync() {
    check_sync("TcpConnection", method_names::TCP_CONNECTION_METHODS);
}

#[test]
fn json_value_methods_sync() {
    check_sync("JsonValue", method_names::JSON_VALUE_METHODS);
}

#[test]
fn path_methods_interp_covered() {
    check_interp_covered_by_registry("Path", method_names::PATH_METHODS);
}

#[test]
fn args_methods_sync() {
    check_sync("Args", method_names::ARGS_METHODS);
}

// ---------------------------------------------------------------------------
// Time types — full sync
// ---------------------------------------------------------------------------

#[test]
fn instant_methods_interp_covered() {
    // Instant has "now" as static (in the registry) but interpreter handles
    // it separately via call_time_type_method, so only check instance methods
    check_interp_covered_by_registry("Instant", method_names::INSTANT_METHODS);
}

#[test]
fn duration_methods_interp_covered() {
    // Duration has static constructors (seconds, millis, etc.) handled separately
    check_interp_covered_by_registry("Duration", method_names::DURATION_METHODS);
}

// ---------------------------------------------------------------------------
// Concurrency types — full sync
// ---------------------------------------------------------------------------

#[test]
fn thread_handle_methods_sync() {
    check_sync("ThreadHandle", method_names::THREAD_HANDLE_METHODS);
}

#[test]
fn sender_methods_sync() {
    check_sync("Sender", method_names::SENDER_METHODS);
}

#[test]
fn receiver_methods_sync() {
    check_sync("Receiver", method_names::RECEIVER_METHODS);
}

#[test]
fn shared_methods_interp_covered() {
    // Shared has "new" as static constructor handled separately
    check_interp_covered_by_registry("Shared", method_names::SHARED_METHODS);
}

#[test]
fn atomic_bool_methods_sync() {
    check_sync("AtomicBool", method_names::ATOMIC_BOOL_METHODS);
}

#[test]
fn atomic_usize_methods_sync() {
    check_sync("AtomicUsize", method_names::ATOMIC_USIZE_METHODS);
}

#[test]
fn atomic_u64_methods_sync() {
    check_sync("AtomicU64", method_names::ATOMIC_U64_METHODS);
}

// ---------------------------------------------------------------------------
// Module methods — full sync
// ---------------------------------------------------------------------------

#[test]
fn fs_module_sync() {
    check_module_sync("fs", method_names::FS_MODULE_METHODS);
}

#[test]
fn net_module_sync() {
    check_module_sync("net", method_names::NET_MODULE_METHODS);
}

#[test]
fn json_module_sync() {
    check_module_sync("json", method_names::JSON_MODULE_METHODS);
}

#[test]
fn time_module_sync() {
    check_module_sync("time", method_names::TIME_MODULE_METHODS);
}

#[test]
fn math_module_sync() {
    check_module_sync("math", method_names::MATH_MODULE_METHODS);
}

#[test]
fn random_module_sync() {
    check_module_sync("random", method_names::RANDOM_MODULE_METHODS);
}

#[test]
fn os_module_sync() {
    check_module_sync("os", method_names::OS_MODULE_METHODS);
}

#[test]
fn io_module_sync() {
    check_module_sync("io", method_names::IO_MODULE_METHODS);
}

#[test]
fn cli_module_sync() {
    check_module_sync("cli", method_names::CLI_MODULE_METHODS);
}
