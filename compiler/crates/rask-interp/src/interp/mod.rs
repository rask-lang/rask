// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The interpreter implementation.
//!
//! This is a tree-walk interpreter that directly evaluates the AST.
//! After desugaring, arithmetic operators become method calls (a + b → a.add(b)),
//! so the interpreter implements these methods on primitive types.

use std::collections::HashMap;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

mod register;
mod monomorphize;
mod call;
mod exec_stmt;
mod assign;
mod eval_expr;
mod pattern;
mod collections;
mod format;
mod operators;
pub(crate) mod overflow;
mod dispatch;

use rask_ast::decl::{BenchmarkDecl, Decl, EnumDecl, FnDecl, StructDecl, TestDecl};
use rask_ast::span::LineMap;
use rask_ast::Span;

use crate::env::Environment;
use crate::resource::ResourceTracker;
use crate::value::Value;

pub(crate) mod binary;

/// Declarations collected during registration.
struct RegisteredProgram {
    entry_fn: Option<FnDecl>,
    tests: Vec<TestDecl>,
    benchmarks: Vec<BenchmarkDecl>,
    test_fns: Vec<FnDecl>,
}

/// Result of running a single test.
#[derive(Debug)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub duration: std::time::Duration,
    pub errors: Vec<String>,
    /// Test was skipped via skip("reason")
    pub skipped: Option<String>,
    /// Whatever the body printed. Captured per-test rather than written straight
    /// through, so the CLI can show it under the test that produced it — and so
    /// both backends render it the same way (#612).
    pub output: String,
}

/// Result of running a single benchmark.
#[derive(Debug)]
pub struct BenchmarkResult {
    pub name: String,
    pub iterations: u64,
    pub total: std::time::Duration,
    pub min: std::time::Duration,
    pub max: std::time::Duration,
    pub mean: std::time::Duration,
    pub median: std::time::Duration,
}

/// The tree-walk interpreter.
pub struct Interpreter {
    /// Variable bindings (scoped).
    pub(crate) env: Environment,
    /// Function declarations by name.
    functions: HashMap<String, FnDecl>,
    /// Enum declarations by name.
    pub(crate) enums: HashMap<String, EnumDecl>,
    /// Struct declarations by name (for @resource checking).
    pub(crate) struct_decls: HashMap<String, StructDecl>,
    /// Nominal newtype name → what it wraps, as written.
    ///
    /// `type NodeId = u64` is transparent to everything except the type checker,
    /// so nothing here needed the target until `reflect.is_flat` had to follow
    /// it — a newtype over a primitive is flat, and answering "not declared" made
    /// it not (#791).
    pub(crate) nominal_targets: HashMap<String, String>,
    /// `type alias X = Y` — the transparent kind, which is the same type under
    /// another spelling. An instance call takes its prefix from the receiver's
    /// *value*, so the alias is long gone by then; a static call takes it from
    /// the spelling, and `Zwibble.make(7)` found nothing named `Zwibble` in
    /// scope. Resolved through here before the static-call path reads the name
    /// (#998).
    pub(crate) transparent_aliases: HashMap<String, String>,
    /// Monomorphized struct declarations (e.g., "Buffer<i32, 256>" -> concrete struct).
    monomorphized_structs: HashMap<String, StructDecl>,
    /// Methods from extend blocks (type_name -> method_name -> FnDecl).
    pub(crate) methods: HashMap<String, HashMap<String, FnDecl>>,
    /// Linear resource tracker.
    pub(crate) resource_tracker: ResourceTracker,
    /// Optional output buffer for capturing stdout (used in tests).
    output_buffer: Option<Arc<Mutex<String>>>,
    /// Command-line arguments passed to the program.
    pub(crate) cli_args: Vec<String>,
    /// Build script state (set when running via `run_build`).
    pub(crate) build_state: Option<crate::build_context::BuildState>,
    /// Source info for error origin tracking (ER15): file name + line map.
    pub(crate) source_info: Option<SourceInfo>,
    /// B1–G4: binary struct metadata for @binary parse/build.
    pub(crate) binary_structs: HashMap<String, binary::BinaryStructMeta>,
    /// Static expression types from the checker, keyed by NodeId. Used to
    /// recover integer widths for overflow checking (type.overflow). Empty
    /// when types weren't supplied (e.g. comptime pre-check paths).
    pub(crate) node_types: HashMap<rask_ast::NodeId, rask_types::Type>,
    /// What each generic function's type parameters resolved to for the call
    /// currently on the stack, innermost last.
    ///
    /// The interpreter doesn't monomorphize, so `T` inside a generic body is
    /// just a name. Anything comptime that asks "what is `T` right now" —
    /// `reflect.fields<T>()` above all — got the literal "T" and gave up (#699).
    /// Inferred from the runtime argument bound to a parameter declared as that
    /// bare name, and scoped like `env`.
    pub(crate) type_bindings: Vec<HashMap<String, String>>,
    /// Type arguments written at the call about to be made, in order.
    ///
    /// `type_bindings` is inferred from the arguments, which answers nothing for
    /// a call that has none: `count<Plain>()` left `T` unbound, so
    /// `reflect.name_of<T>()` returned the string "T" while native said "Plain"
    /// (#968). The parser folds written type arguments into the callee's name,
    /// so they're read off there and parked here for the callee to bind.
    ///
    /// Taken, not read — set immediately before the call, after the arguments
    /// are evaluated, so a nested call during argument evaluation can't pick it
    /// up and the next call can't inherit a stale one.
    pub(crate) pending_type_args: Option<Vec<String>>,
    /// Nested Rask calls currently on the host stack.
    ///
    /// An interpreted call costs about 30 KB of Rust stack — `eval_expr` is one
    /// match with 80 arms and Rust sizes the frame for the union of every arm's
    /// locals — so the host stack runs out at a few hundred Rask frames. It used
    /// to run out by overflowing: SIGABRT, no message, no exit code (#759).
    /// Counted here so the limit is reported instead.
    pub(crate) call_depth: usize,
    /// ER31a: `try` sites whose error the checker decided to wrap in a variant
    /// of the enclosing function's error enum, keyed by the `try` expression.
    pub(crate) error_wraps: HashMap<rask_ast::NodeId, rask_types::ErrorWrap>,
    /// ER14a: `??` sites that keep the optional shape instead of unwrapping.
    pub(crate) fallback_keeps_shape: std::collections::HashSet<rask_ast::NodeId>,
    /// ER16a: `try` node → the postfix-chain step it attaches to, when that
    /// isn't the operand itself. `try read_file(p).len()` propagates at the
    /// call and hands `.len()` the payload.
    pub(crate) try_chain_placement: HashMap<rask_ast::NodeId, rask_ast::NodeId>,
    /// ER16a: the `try` whose propagation is still owed, and the step it waits
    /// for. Armed when a `try` node is evaluated, discharged at that step.
    pub(crate) pending_try_step: Option<(rask_ast::NodeId, rask_ast::NodeId)>,
    /// Final values of `mutate` parameters from the most recent user-function
    /// call, keyed by parameter index (mem.parameters/PM2). The call site reads
    /// this to write each value back to its argument place. Cleared before every
    /// call so stale entries can't leak into an unrelated call's arguments.
    pub(crate) mutate_writebacks: Vec<(usize, Value)>,
}

/// Source location info for computing error origins (ER15).
#[derive(Clone)]
pub struct SourceInfo {
    pub file_name: String,
    pub line_map: LineMap,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            monomorphized_structs: HashMap::new(),
            methods: HashMap::new(),
            nominal_targets: HashMap::new(),
            transparent_aliases: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: None,
            cli_args: vec![],
            build_state: None,
            source_info: None,
            binary_structs: HashMap::new(),
            node_types: HashMap::new(),
            type_bindings: Vec::new(),
            pending_type_args: None,
            call_depth: 0,
            error_wraps: HashMap::new(),
            try_chain_placement: HashMap::new(),
            pending_try_step: None,
            fallback_keeps_shape: std::collections::HashSet::new(),
            mutate_writebacks: Vec::new(),
        }
    }

    pub fn with_args(args: Vec<String>) -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            monomorphized_structs: HashMap::new(),
            methods: HashMap::new(),
            nominal_targets: HashMap::new(),
            transparent_aliases: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: None,
            cli_args: args,
            binary_structs: HashMap::new(),
            node_types: HashMap::new(),
            type_bindings: Vec::new(),
            pending_type_args: None,
            call_depth: 0,
            error_wraps: HashMap::new(),
            try_chain_placement: HashMap::new(),
            pending_try_step: None,
            fallback_keeps_shape: std::collections::HashSet::new(),
            build_state: None,
            source_info: None,
            mutate_writebacks: Vec::new(),
        }
    }

    /// Returns interpreter and output buffer reference.
    pub fn with_captured_output() -> (Self, Arc<Mutex<String>>) {
        let buffer = Arc::new(Mutex::new(String::new()));
        let interp = Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            monomorphized_structs: HashMap::new(),
            methods: HashMap::new(),
            nominal_targets: HashMap::new(),
            transparent_aliases: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: Some(buffer.clone()),
            cli_args: vec![],
            build_state: None,
            source_info: None,
            binary_structs: HashMap::new(),
            node_types: HashMap::new(),
            type_bindings: Vec::new(),
            pending_type_args: None,
            call_depth: 0,
            error_wraps: HashMap::new(),
            try_chain_placement: HashMap::new(),
            pending_try_step: None,
            fallback_keeps_shape: std::collections::HashSet::new(),
            mutate_writebacks: Vec::new(),
        };
        (interp, buffer)
    }

    /// Inject `cfg` build configuration into the interpreter environment (CT11-CT16).
    /// Set source info for error origin tracking (ER15).
    pub fn set_source_info(&mut self, file_name: &str, source: &str) {
        self.source_info = Some(SourceInfo {
            file_name: file_name.to_string(),
            line_map: LineMap::new(source),
        });
    }

    /// Compute an error origin string like `"file.rk:42"` from a span.
    /// Follow a transparent `type alias` chain to the type it names.
    ///
    /// Bounded rather than trusting the chain to be acyclic: `type alias A = B`
    /// with `type alias B = A` is a resolution error, not something a runtime
    /// loop should hang on.
    pub(crate) fn resolve_transparent_alias(&self, name: &str) -> String {
        let mut current = name;
        for _ in 0..16 {
            match self.transparent_aliases.get(current) {
                Some(target) if target != current => current = target,
                _ => break,
            }
        }
        current.to_string()
    }

    pub(crate) fn origin_string(&self, span: Span) -> Arc<str> {
        if let Some(info) = &self.source_info {
            let (line, _) = info.line_map.offset_to_line_col(span.start);
            Arc::from(format!("{}:{}", info.file_name, line))
        } else {
            Arc::from("<unknown>")
        }
    }

    /// What a failed task's message should say when *user code* reads it back
    /// out of `JoinError.Panicked(msg)`.
    ///
    /// Not `Display`: that renders a panic as `panic: boom`, and
    /// `JoinError.message()` wraps it again as `task panicked: panic: boom`.
    /// The reporter's own wording belongs at print time, not baked into a
    /// string a program is going to print itself (#748). The location is what
    /// you want when a background task dies, so that's what it carries —
    /// `file:line:col: boom`, matching native.
    pub(crate) fn task_failure_message(&self, diag: &RuntimeDiagnostic) -> String {
        let RuntimeError::Panic(msg) = &diag.error else {
            return format!("{}", diag);
        };
        match &self.source_info {
            Some(info) => {
                // file:line, no column — see the note in the runtime's
                // rask_panic_at: the two backends point their columns at
                // different sub-expressions, and the line is the useful half.
                let (line, _) = info.line_map.offset_to_line_col(diag.span.start);
                format!("{}:{}: {}", info.file_name, line, msg)
            }
            None => msg.clone(),
        }
    }

    /// Supply the checker's static expression types, enabling width-aware
    /// integer overflow checks (type.overflow). Without this the interpreter
    /// falls back to unchecked i64 arithmetic.
    pub fn set_node_types(&mut self, node_types: HashMap<rask_ast::NodeId, rask_types::Type>) {
        self.node_types = node_types;
    }

    /// The nominal type name of a runtime value, for matching against a generic
    /// function's type parameter. `None` for values with no name to give.
    pub(crate) fn runtime_type_name(value: &Value) -> Option<String> {
        match value {
            Value::Struct(s) => Some(s.lock().unwrap().name.clone()),
            Value::Enum { name, .. } => Some(name.clone()),
            Value::Bool(_) => Some("bool".to_string()),
            Value::Int(_, _) => Some("i64".to_string()),
            Value::Float(_, _) => Some("f64".to_string()),
            Value::Char(_) => Some("char".to_string()),
            Value::String(_) => Some("string".to_string()),
            _ => None,
        }
    }

    /// What `name` resolved to for the innermost generic call that bound it, or
    /// `name` itself when nothing did.
    pub(crate) fn resolve_type_param(&self, name: &str) -> String {
        for frame in self.type_bindings.iter().rev() {
            if let Some(concrete) = frame.get(name) {
                return concrete.clone();
            }
        }
        name.to_string()
    }

    /// How many optional layers a container's Nth type argument declares —
    /// `Vec<i32?>` at index 0 answers 1, `Map<string, i32??>` at index 1
    /// answers 2. `None` when the receiver's type isn't a resolved container.
    pub(crate) fn container_elem_option_depth(
        &self,
        node_id: rask_ast::NodeId,
        index: usize,
    ) -> Option<usize> {
        use rask_types::{GenericArg, Type};
        fn depth(ty: &Type) -> usize {
            match ty {
                Type::Result { ok, err } if **err == Type::None => 1 + depth(ok),
                _ => 0,
            }
        }
        let args = match self.node_types.get(&node_id)? {
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args,
            _ => return None,
        };
        let GenericArg::Type(inner) = args.get(index)? else {
            return None;
        };
        Some(depth(inner))
    }

    /// ER31a: supply the `try` sites whose error gets wrapped in the enclosing
    /// function's error enum. Without this, those errors propagate unwrapped and
    /// the caller's `match` on the boundary enum finds a variant it doesn't know.
    pub fn set_error_wraps(
        &mut self,
        wraps: HashMap<rask_ast::NodeId, rask_types::ErrorWrap>,
    ) {
        self.error_wraps = wraps;
    }

    /// ER16a: supply the `try` sites that attach to a step inside a postfix
    /// chain rather than to the whole operand.
    pub fn set_try_chain_placement(
        &mut self,
        placement: HashMap<rask_ast::NodeId, rask_ast::NodeId>,
    ) {
        self.try_chain_placement = placement;
    }

    /// ER14a: supply the `??` sites whose right side is still wrapped, so a
    /// present left operand is handed back with its layer intact.
    pub fn set_fallback_keeps_shape(
        &mut self,
        sites: std::collections::HashSet<rask_ast::NodeId>,
    ) {
        self.fallback_keeps_shape = sites;
    }

    pub fn inject_cfg(&mut self, cfg: &rask_comptime::CfgConfig) {
        let mut fields = IndexMap::new();
        fields.insert("os".to_string(), Value::String(Arc::new(Mutex::new(cfg.os.clone()))));
        fields.insert("arch".to_string(), Value::String(Arc::new(Mutex::new(cfg.arch.clone()))));
        fields.insert("env".to_string(), Value::String(Arc::new(Mutex::new(cfg.env.clone()))));
        fields.insert("profile".to_string(), Value::String(Arc::new(Mutex::new(cfg.profile.clone()))));
        fields.insert("debug".to_string(), Value::Bool(cfg.profile == "debug"));
        fields.insert("features".to_string(), Value::vec(
            cfg.features.iter().map(|f| Value::String(Arc::new(Mutex::new(f.clone())))).collect(),
        ));
        self.env.define("cfg".to_string(), Value::new_struct(
            "Cfg".to_string(),
            fields,
            None,
        ));
    }

    /// Clones function/enum/method tables and captured environment for spawned thread.
    pub(crate) fn spawn_child(&self, captured_vars: HashMap<String, Value>) -> Self {
        let mut child = Interpreter::new();
        child.functions = self.functions.clone();
        child.enums = self.enums.clone();
        child.struct_decls = self.struct_decls.clone();
        child.methods = self.methods.clone();
        child.node_types = self.node_types.clone();
        child.error_wraps = self.error_wraps.clone();
        child.try_chain_placement = self.try_chain_placement.clone();
        child.fallback_keeps_shape = self.fallback_keeps_shape.clone();
        // A task that panics reports `file:line:col`, so the child needs the
        // source it's running (#748). Without this a spawned task's message
        // came back as bare text while the main thread's carried a location.
        child.source_info = self.source_info.clone();
        for (name, value) in captured_vars {
            child.env.define(name, value);
        }
        child
    }

    /// Spawn an OS thread from a closure (Thread.spawn).
    pub(crate) fn spawn_os_thread(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        use crate::value::ThreadHandleInner;

        if args.is_empty() {
            return Err(RuntimeError::TypeError(
                "Thread.spawn requires a closure argument".to_string(),
            ));
        }

        let closure = &args[0];
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                if !params.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "Thread.spawn closure must take no parameters".to_string(),
                    ));
                }

                let body = body.clone();
                let captured = captured_env.clone();
                let child = self.spawn_child(captured);

                let join_handle = crate::spawn_interp_thread(move || {
                    let mut interp = child;
                    match interp.eval_expr(&body) {
                        Ok(val) => Ok(val),
                        Err(diag) if matches!(diag.error, RuntimeError::Return(_)) => {
                            match diag.error {
                                RuntimeError::Return(val) => Ok(val),
                                _ => unreachable!("checked above"),
                            }
                        }
                        Err(diag) => Err(interp.task_failure_message(&diag)),
                    }
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                    receiver: Mutex::new(None),
                    task_id: crate::value::next_task_id(),
                })))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "Thread.spawn expects a closure, got {}",
                closure.type_name()
            ))),
        }
    }

    /// Spawn an async task from a closure (spawn() in using Multitasking).
    /// In interpreter: uses OS thread but returns TaskHandle for type distinction.
    pub(crate) fn spawn_async_task(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        use crate::value::ThreadHandleInner;

        if args.is_empty() {
            return Err(RuntimeError::TypeError(
                "spawn() requires a closure argument".to_string(),
            ));
        }

        // Check for active runtime slot (CC3 fallback)
        if crate::value::ACTIVE_RUNTIME.read().unwrap().is_none() {
            return Err(RuntimeError::Panic(
                "RUNTIME PANIC: spawn() called with no active `using Multitasking` scope\n\
                 Install a `using Multitasking { ... }` block that encloses the call.".to_string(),
            ));
        }

        let closure = &args[0];
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                if !params.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "spawn() closure must take no parameters".to_string(),
                    ));
                }

                let body = body.clone();
                let captured = captured_env.clone();
                let child = self.spawn_child(captured);

                let join_handle = crate::spawn_interp_thread(move || {
                    let mut interp = child;
                    match interp.eval_expr(&body) {
                        Ok(val) => Ok(val),
                        Err(diag) if matches!(diag.error, RuntimeError::Return(_)) => {
                            match diag.error {
                                RuntimeError::Return(val) => Ok(val),
                                _ => unreachable!("checked above"),
                            }
                        }
                        Err(diag) => Err(interp.task_failure_message(&diag)),
                    }
                });

                // Return TaskHandle (not ThreadHandle) for type distinction
                let handle_inner = Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                    receiver: Mutex::new(None),
                    task_id: crate::value::next_task_id(),
                });

                // Register handle for affine tracking (conc.async/H1)
                let ptr = Arc::as_ptr(&handle_inner) as usize;
                self.resource_tracker.register_handle(ptr, "TaskHandle", self.env.scope_depth());

                Ok(Value::TaskHandle(handle_inner))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "spawn() expects a closure, got {}",
                closure.type_name()
            ))),
        }
    }

    /// Spawn a thread pool task from a closure (ThreadPool.spawn).
    pub(crate) fn spawn_pool_task(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        use crate::value::{PoolTask, ThreadHandleInner};

        if args.is_empty() {
            return Err(RuntimeError::TypeError(
                "ThreadPool.spawn requires a closure argument".to_string(),
            ));
        }

        let closure = &args[0];
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                if !params.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "ThreadPool.spawn closure must take no parameters".to_string(),
                    ));
                }

                // Check for thread pool context
                let pool = self.env.get("__thread_pool").cloned();
                let pool = match pool {
                    Some(Value::ThreadPool(p)) => p,
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "ThreadPool.spawn requires `using ThreadPool` context".to_string(),
                        ))
                    }
                };

                let body = body.clone();
                let captured = captured_env.clone();
                let child = self.spawn_child(captured);

                let (result_tx, result_rx) = mpsc::sync_channel::<Result<Value, String>>(1);

                let task = PoolTask {
                    work: Box::new(move || {
                        let mut interp = child;
                        match interp.eval_expr(&body) {
                            Ok(val) => {
                                let _ = result_tx.send(Ok(val));
                            }
                            Err(diag) => match diag.error {
                                RuntimeError::Return(val) => {
                                    let _ = result_tx.send(Ok(val));
                                }
                                _ => {
                                    let _ = result_tx.send(
                                        Err(interp.task_failure_message(&diag)),
                                    );
                                }
                            },
                        }
                    }),
                };

                let sender = pool.sender.lock().unwrap();
                if let Some(ref tx) = *sender {
                    tx.send(task).map_err(|_| {
                        RuntimeError::ResourceClosed {
                            resource_type: "ThreadPool".to_string(),
                            operation: "spawn on".to_string(),
                        }
                    })?;
                } else {
                    return Err(RuntimeError::TypeError(
                        "thread pool is shut down".to_string(),
                    ));
                }

                let join_handle = crate::spawn_interp_thread(move || {
                    result_rx
                        .recv()
                        .unwrap_or(Err("thread pool task dropped".to_string()))
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                    receiver: Mutex::new(None),
                    task_id: crate::value::next_task_id(),
                })))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "ThreadPool.spawn expects a closure, got {}",
                closure.type_name()
            ))),
        }
    }

    /// Divert print output into a fresh buffer, handing back whatever was
    /// installed before. Restore with `restore_output_capture`.
    pub(crate) fn begin_output_capture(&mut self) -> Option<Arc<Mutex<String>>> {
        let previous = self.output_buffer.take();
        self.output_buffer = Some(Arc::new(Mutex::new(String::new())));
        previous
    }

    /// Put back `previous` and return what was captured meanwhile. When a buffer
    /// was already installed the captured text is appended to it instead, so an
    /// outer capture still sees everything.
    pub(crate) fn restore_output_capture(
        &mut self,
        previous: Option<Arc<Mutex<String>>>,
    ) -> String {
        let captured = self
            .output_buffer
            .take()
            .map(|b| b.lock().unwrap().clone())
            .unwrap_or_default();
        if let Some(outer) = &previous {
            outer.lock().unwrap().push_str(&captured);
        }
        self.output_buffer = previous;
        captured
    }

    fn write_output(&self, s: &str) {
        if let Some(buf) = &self.output_buffer {
            buf.lock().unwrap().push_str(s);
        } else {
            print!("{}", s);
        }
    }

    fn write_output_ln(&self) {
        if let Some(buf) = &self.output_buffer {
            buf.lock().unwrap().push('\n');
        } else {
            println!();
        }
    }

    fn is_resource_type(&self, name: &str) -> bool {
        if name == "File" {
            return true;
        }
        self.struct_decls
            .get(name)
            .map(|s| s.attrs.iter().any(|a| a == "resource"))
            .unwrap_or(false)
    }

    pub(crate) fn get_resource_id(&self, value: &Value) -> Option<u64> {
        match value {
            Value::Struct(ref s) => s.lock().unwrap().resource_id,
            Value::File(rc) => {
                let ptr = Arc::as_ptr(rc) as usize;
                self.resource_tracker.lookup_file_id(ptr)
            }
            Value::TaskHandle(h) | Value::ThreadHandle(h) => {
                let ptr = Arc::as_ptr(h) as usize;
                self.resource_tracker.lookup_handle_id(ptr)
            }
            _ => None,
        }
    }

    /// Handles nested values like Result.Ok(file) or Result.Err(FileError{file}).
    fn transfer_resource_to_scope(&mut self, value: &Value, new_depth: usize) {
        // A program with no live resources has nothing to hand over, and this
        // walks aggregates — so the common case doesn't pay for the search.
        if self.resource_tracker.is_empty() {
            return;
        }
        match value {
            Value::File(rc) => {
                let ptr = Arc::as_ptr(rc) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker.transfer_to_scope(id, new_depth);
                }
            }
            Value::TaskHandle(h) | Value::ThreadHandle(h) => {
                let ptr = Arc::as_ptr(h) as usize;
                if let Some(id) = self.resource_tracker.lookup_handle_id(ptr) {
                    self.resource_tracker.transfer_to_scope(id, new_depth);
                }
            }
            Value::Struct(ref s) => {
                let (id, fields) = {
                    let data = s.lock().unwrap();
                    (data.resource_id, data.fields.values().cloned().collect::<Vec<_>>())
                };
                if let Some(id) = id {
                    self.resource_tracker.transfer_to_scope(id, new_depth);
                }
                // A struct that isn't itself a resource can still hold one —
                // `return Wrap { conn: conn }` hands it over just as directly.
                for field in &fields {
                    self.transfer_resource_to_scope(field, new_depth);
                }
            }
            Value::Enum { fields, .. } => {
                for field in fields {
                    self.transfer_resource_to_scope(field, new_depth);
                }
            }
            // A tuple is a `Vec` at runtime, and `return (request, responder)`
            // hands the resource to the caller the same way a struct field
            // does. Without this the callee's scope exit read it as a leak, and
            // native — which has no runtime tracker — disagreed (#792). A real
            // `Vec` can't hold a resource at all (mem.linear/RC1, RC3), so
            // walking one costs nothing and finds nothing.
            Value::Vec(items) => {
                let snapshot: Vec<Value> = items.lock().unwrap().items.clone();
                for item in &snapshot {
                    self.transfer_resource_to_scope(item, new_depth);
                }
            }
            _ => {}
        }
    }

    /// Register external package names so `pkg.func()` works at runtime.
    pub fn register_packages(&mut self, names: &[String]) {
        for name in names {
            self.env.define(name.clone(), Value::Package(name.clone()));
        }
    }

    pub fn run(&mut self, decls: &[Decl]) -> Result<Value, RuntimeDiagnostic> {
        let registered = self.register_declarations(decls)
            .map_err(|e| RuntimeDiagnostic::new(e, Span::new(0, 0)))?;

        if let Some(entry) = registered.entry_fn {
            // On a big stack, like every spawned task. `main` ran on the process
            // main thread's 8 MiB, so it managed ~245 nested Rask calls where a
            // task got ~450 on its 16 MiB — the same program, a different depth
            // depending on which thread ran it (#759).
            let value = crate::on_interp_stack(|| self.call_function(&entry, vec![]));
            // O4: a detached task's panic has to reach stderr, and a reaper
            // racing process exit doesn't satisfy that — the report just
            // vanishes, which is the failure O4 exists to prevent. Wait here,
            // whichever way main finished.
            crate::join_detached_reapers();
            // Before the `?`, so a program that ends in an error still reports
            // its store stats.
            crate::rack::print_stats();
            let value = value?;
            // struct.targets/EX4: an error out of main is exit status 1, not 0.
            // A `try` that propagates already lands in the error path; an
            // explicit `return SomeError` came back as an ordinary value and
            // the process reported success (#345).
            if let Some(err) = Self::main_error_return(entry.ret_ty.as_deref(), &value) {
                let msg = self.describe_error_value(err);
                return Err(RuntimeDiagnostic::new(
                    RuntimeError::MainReturnedError(msg),
                    entry.span,
                ));
            }
            Ok(value)
        } else {
            Err(RuntimeDiagnostic::new(RuntimeError::NoEntryPoint, Span::new(0, 0)))
        }
    }

    /// The error `main` returned, if it returned one. `ret_ty` is main's
    /// declared return type — without a `T or E` there's no error branch and
    /// every value is a success.
    fn main_error_return<'v>(ret_ty: Option<&str>, value: &'v Value) -> Option<&'v Value> {
        // Desugar rewrites `T or E` to `Result<T, E>`, but the source spelling
        // survives in some paths — accept either.
        let ret_ty = ret_ty?.trim();
        let err_branch = match ret_ty.split_once(" or ") {
            Some((_, e)) => e.trim(),
            None => {
                let inner = ret_ty.strip_prefix("Result<")?.strip_suffix('>')?;
                Self::split_top_level_comma(inner)?.1.trim()
            }
        };
        match value {
            Value::Enum { name, variant, fields, .. } if name == "Result" => {
                (variant == "Err").then(|| fields.first().unwrap_or(&Value::Unit))
            }
            Value::Unit => None,
            other => {
                // A bare error value returned without a Result wrapper. It's
                // the error branch when its type is named there — `void or Fail`
                // returning a `Fail`, or one arm of a `A | B` union.
                let name = match other {
                    Value::Struct(s) => s.lock().unwrap().name.clone(),
                    Value::Enum { name, .. } => {
                        name.split('.').next().unwrap_or(name).to_string()
                    }
                    Value::Nominal { type_name, .. } => type_name.clone(),
                    _ => return None,
                };
                err_branch
                    .split('|')
                    .any(|e| e.trim() == name)
                    .then_some(other)
            }
        }
    }

    /// Split `Result<...>`'s arguments at the comma that separates ok from err,
    /// ignoring commas inside a nested generic.
    fn split_top_level_comma(s: &str) -> Option<(&str, &str)> {
        let mut depth = 0usize;
        for (i, c) in s.char_indices() {
            match c {
                '<' | '(' | '[' => depth += 1,
                '>' | ')' | ']' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => return Some((&s[..i], &s[i + 1..])),
                _ => {}
            }
        }
        None
    }

    /// Human-readable text for an error value: its own `message()` when the
    /// type defines one, otherwise the value as printed.
    fn describe_error_value(&mut self, err: &Value) -> String {
        let owned = err.clone();
        if let Ok(Value::String(s)) = self.call_method(owned.clone(), "message", vec![]) {
            return s.lock().unwrap().clone();
        }
        format!("{}", owned)
    }

    /// Run a build script: register declarations, find `func build(ctx)`,
    /// call it with the BuildContext value. Sets `build_state` so method
    /// dispatch can accumulate link flags and other state.
    pub fn run_build(
        &mut self,
        decls: &[Decl],
        state: crate::build_context::BuildState,
    ) -> Result<Value, RuntimeDiagnostic> {
        let ctx_value = state.to_value();
        self.build_state = Some(state);

        let registered = self.register_declarations(decls)
            .map_err(|e| RuntimeDiagnostic::new(e, Span::new(0, 0)))?;

        // Find func build — it's the entry point for build scripts
        let build_fn = registered.entry_fn
            .or_else(|| self.functions.get("build").cloned())
            .ok_or_else(|| RuntimeDiagnostic::new(
                RuntimeError::Generic("build.rk has no func build()".into()),
                Span::new(0, 0),
            ))?;

        // If func build takes a parameter, pass ctx; otherwise call with no args
        if build_fn.params.is_empty() {
            self.call_function(&build_fn, vec![])
        } else {
            self.call_function(&build_fn, vec![ctx_value])
        }
    }

    /// Take the build state after a build script finishes.
    pub fn take_build_state(&mut self) -> Option<crate::build_context::BuildState> {
        self.build_state.take()
    }

    /// Run all tests in the program (test blocks + @test functions).
    /// Does NOT require an entry point.
    pub fn run_tests(&mut self, decls: &[Decl], filter: Option<&str>) -> Vec<TestResult> {
        let registered = match self.register_declarations(decls) {
            Ok(r) => r,
            Err(e) => {
                return vec![TestResult {
                    name: "<registration>".to_string(),
                    passed: false,
                    duration: std::time::Duration::ZERO,
                    errors: vec![format!("{}", e)],
                    skipped: None,
                    output: String::new(),
                }];
            }
        };

        // Same stack as `main` and every spawned task, so a test's recursion
        // depth doesn't depend on which entry point ran it (#759).
        crate::on_interp_stack(|| {
            let mut results = Vec::new();

            for test_decl in &registered.tests {
                if let Some(pat) = filter {
                    if !test_decl.name.contains(pat) {
                        continue;
                    }
                }
                results.push(self.run_single_test(&test_decl.name, &test_decl.body));
            }

            for test_fn in &registered.test_fns {
                if let Some(pat) = filter {
                    if !test_fn.name.contains(pat) {
                        continue;
                    }
                }
                results.push(self.run_test_function(test_fn));
            }

            results
        })
    }

    /// Run all benchmarks in the program.
    pub fn run_benchmarks(&mut self, decls: &[Decl], filter: Option<&str>) -> Vec<BenchmarkResult> {
        let registered = match self.register_declarations(decls) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Registration error: {}", e);
                return vec![];
            }
        };

        let mut results = Vec::new();

        for bench in &registered.benchmarks {
            if let Some(pat) = filter {
                if !bench.name.contains(pat) {
                    continue;
                }
            }
            results.push(self.run_single_benchmark(&bench.name, &bench.body));
        }

        results
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// A runtime error.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("undefined variable `{0}`")]
    UndefinedVariable(String),

    #[error("undefined function `{0}`")]
    UndefinedFunction(String),

    #[error("{0}")]
    TypeError(String),

    #[error("division by zero; check divisor before dividing")]
    DivisionByZero,

    #[error("{0}")]
    IntegerOverflow(String),

    #[error("expected {expected} argument{}, got {got}", if *.expected == 1 { "" } else { "s" })]
    ArityMismatch { expected: usize, got: usize },

    #[error("no method `{method}` on type `{ty}`")]
    NoSuchMethod { ty: String, method: String },

    #[error("no field `{field}` on type `{ty}`")]
    NoSuchField { ty: String, field: String },

    #[error("index {index} out of bounds (length is {len})")]
    IndexOutOfBounds { index: i64, len: usize },

    #[error("resource is closed; cannot {operation} a closed {resource_type}")]
    ResourceClosed { resource_type: String, operation: String },

    #[error("panic: {0}")]
    Panic(String),

    #[error("no matching arm in match; add a wildcard `_` arm to handle all cases")]
    NoMatchingArm,

    #[error("multiple @entry functions found; only one `func main()` or `@entry` per program")]
    MultipleEntryPoints,

    #[error("no entry point found; add `func main()` or use `@entry`")]
    NoEntryPoint,

    /// The interpreter ran out of call depth. Reported rather than left to
    /// overflow the host stack, which killed the process with nothing printed.
    #[error("recursion too deep: {depth} nested calls, innermost `{function}`")]
    RecursionTooDeep { function: String, depth: usize },

    /// struct.targets/EX4: main returned the error branch of its `T or E`.
    #[error("{0}")]
    MainReturnedError(String),

    #[error("{0}")]
    Generic(String),

    #[error("exit with code {0}")]
    Exit(i32),

    // Control flow (not actual errors)
    #[error("return")]
    Return(Value),

    /// Break value plus the label it targets, if any (CF23/CF25). A loop only
    /// absorbs an unlabeled break or one naming itself; anything else keeps
    /// unwinding to the loop that owns the label.
    #[error("break")]
    Break(Value, Option<String>),

    /// Continue plus the label it targets, if any (CF24).
    #[error("continue")]
    Continue(Option<String>),

    /// Error propagation via try operator
    #[error("try error")]
    TryError(Value),

    /// `x!` on an absent optional (type.optionals/OPT13).
    #[error("! on a value that was absent")]
    ForcedAbsent,

    /// `r!` on an error result (type.errors/ER15). A separate case from
    /// ForcedAbsent because they are different mistakes: one had nothing there,
    /// the other had a failure it threw away. Both used to report "value was
    /// None", which for the error case names something that never happened.
    ///
    /// Carries the error's own `message()`. ER15 says `!` panics *using* it,
    /// and ctrl.panic/F3 wants a panic message to be a function of the failing
    /// operation's operands — here the operand is the error, and every error
    /// type has a `message()` (that's what E0344 enforces), so there is always
    /// something to print. Reporting only "was an error" threw away the one
    /// thing the reader wanted and had in hand (#1009).
    #[error("! on a value that was an error: {0}")]
    ForcedError(String),

    /// Assertion failed (assert expr) — stops test immediately
    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    /// Check failed (check expr) — test continues, marked failed
    #[error("check failed: {0}")]
    CheckFailed(String),

    /// Test skipped via skip("reason")
    #[error("skipped: {0}")]
    TestSkipped(String),

    /// Test expects failure via expect_fail()
    #[error("expect_fail")]
    TestExpectFail,
}

impl RuntimeError {
    /// Is this the program panicking, as opposed to failing some other way?
    ///
    /// struct.targets/EX4 puts a panic at exit 101 and an error returned from
    /// `main` at exit 1, and the distinction is the point: 101 says "a bug",
    /// 1 says "the program said no". Only `Panic` was counted here, so an
    /// overflow, a divide by zero, a forced `x!` on `none` and a failed
    /// `assert` all exited 1 on the interpreter while native exited 101 for
    /// every one of them.
    ///
    /// Panicking is what the *spec* says these do — OV1–OV4 and SH1 say
    /// "panics", OPT13 says `x!` panics when there's nothing to force — so the
    /// exit code follows the rule rather than the enum variant that happens to
    /// carry the message.
    ///
    /// Deliberately not here: `MainReturnedError` (EX4's exit-1 case),
    /// `UndefinedVariable`, `TypeError`, `NoSuchMethod` and friends — those are
    /// checker gaps surfacing at runtime, not the program panicking, and giving
    /// them 101 would say the program hit a bug in itself when it didn't.
    pub fn is_panic(&self) -> bool {
        matches!(
            self,
            RuntimeError::Panic(_)
                | RuntimeError::IntegerOverflow(_)
                | RuntimeError::DivisionByZero
                | RuntimeError::IndexOutOfBounds { .. }
                | RuntimeError::ForcedAbsent
                | RuntimeError::ForcedError(_)
                | RuntimeError::NoMatchingArm
                | RuntimeError::ResourceClosed { .. }
                | RuntimeError::AssertionFailed(_)
        )
    }
}

/// Runtime error with source location for diagnostic display.
#[derive(Debug)]
pub struct RuntimeDiagnostic {
    pub error: RuntimeError,
    pub span: Span,
}

impl RuntimeDiagnostic {
    pub fn new(error: RuntimeError, span: Span) -> Self {
        Self { error, span }
    }
}

impl std::fmt::Display for RuntimeDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for RuntimeDiagnostic {}
