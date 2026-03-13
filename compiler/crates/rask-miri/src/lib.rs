// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR interpreter for compile-time function evaluation (CTFE).
//!
//! Executes MIR basic blocks directly. Same semantics as compiled code.
//! Uses `StdlibProvider` for dispatch: `PureStdlib` for comptime (no I/O),
//! `RealStdlib` (future) for scripting.

mod eval;
pub mod intrinsics;
pub mod memory;
pub mod stdlib;

use std::collections::HashMap;
use std::fmt;

use rask_mir::{LocalId, MirFunction, StructLayoutId};
use rask_mono::{StructLayout, EnumLayout};

pub use stdlib::{PureStdlib, StdlibProvider};

// ── Values ──────────────────────────────────────────────────────────

/// Runtime value in the MIR interpreter.
#[derive(Debug, Clone)]
pub enum MiriValue {
    Unit,
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),
    Array(Vec<MiriValue>),
    Tuple(Vec<MiriValue>),
    Struct {
        layout_id: StructLayoutId,
        fields: Vec<MiriValue>,
    },
    Enum {
        tag: u64,
        payload: Option<Box<MiriValue>>,
    },
    /// Function pointer (by name).
    FuncPtr(String),
}

impl MiriValue {
    /// Extract as bool, or error.
    pub fn as_bool(&self) -> Result<bool, MiriError> {
        match self {
            MiriValue::Bool(b) => Ok(*b),
            MiriValue::I64(v) => Ok(*v != 0),
            _ => Err(MiriError::UnsupportedOperation(
                format!("expected bool, got {self:?}"),
            )),
        }
    }

    /// Try to convert to i64 (for numeric casts).
    pub fn to_i64(&self) -> Option<i64> {
        match self {
            MiriValue::I8(v) => Some(*v as i64),
            MiriValue::I16(v) => Some(*v as i64),
            MiriValue::I32(v) => Some(*v as i64),
            MiriValue::I64(v) => Some(*v),
            MiriValue::U8(v) => Some(*v as i64),
            MiriValue::U16(v) => Some(*v as i64),
            MiriValue::U32(v) => Some(*v as i64),
            MiriValue::U64(v) => Some(*v as i64),
            MiriValue::F32(v) => Some(*v as i64),
            MiriValue::F64(v) => Some(*v as i64),
            MiriValue::Bool(v) => Some(if *v { 1 } else { 0 }),
            MiriValue::Char(v) => Some(*v as i64),
            _ => None,
        }
    }

    /// Try to convert to u64 (for numeric casts).
    pub fn to_u64(&self) -> Option<u64> {
        match self {
            MiriValue::I8(v) => Some(*v as u64),
            MiriValue::I16(v) => Some(*v as u64),
            MiriValue::I32(v) => Some(*v as u64),
            MiriValue::I64(v) => Some(*v as u64),
            MiriValue::U8(v) => Some(*v as u64),
            MiriValue::U16(v) => Some(*v as u64),
            MiriValue::U32(v) => Some(*v as u64),
            MiriValue::U64(v) => Some(*v),
            MiriValue::F32(v) => Some(*v as u64),
            MiriValue::F64(v) => Some(*v as u64),
            MiriValue::Bool(v) => Some(if *v { 1 } else { 0 }),
            MiriValue::Char(v) => Some(*v as u64),
            _ => None,
        }
    }

    /// Try to convert to f64 (for numeric casts).
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            MiriValue::I8(v) => Some(*v as f64),
            MiriValue::I16(v) => Some(*v as f64),
            MiriValue::I32(v) => Some(*v as f64),
            MiriValue::I64(v) => Some(*v as f64),
            MiriValue::U8(v) => Some(*v as f64),
            MiriValue::U16(v) => Some(*v as f64),
            MiriValue::U32(v) => Some(*v as f64),
            MiriValue::U64(v) => Some(*v as f64),
            MiriValue::F32(v) => Some(*v as f64),
            MiriValue::F64(v) => Some(*v),
            _ => None,
        }
    }

    /// Serialize to bytes for embedding in data sections.
    /// Returns None for types that can't be statically embedded.
    pub fn serialize(&self) -> Option<Vec<u8>> {
        match self {
            MiriValue::Unit => Some(vec![]),
            MiriValue::Bool(v) => Some(vec![*v as u8]),
            MiriValue::I8(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::I16(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::I32(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::I64(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::U8(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::U16(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::U32(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::U64(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::F32(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::F64(v) => Some(v.to_le_bytes().to_vec()),
            MiriValue::Char(v) => Some((*v as u32).to_le_bytes().to_vec()),
            MiriValue::String(v) => Some(v.as_bytes().to_vec()),
            MiriValue::Array(elems) => {
                let mut bytes = Vec::new();
                for elem in elems {
                    bytes.extend(elem.serialize()?);
                }
                Some(bytes)
            }
            MiriValue::Tuple(fields) => {
                let mut bytes = Vec::new();
                for field in fields {
                    bytes.extend(field.serialize()?);
                }
                Some(bytes)
            }
            MiriValue::Struct { fields, .. } => {
                let mut bytes = Vec::new();
                for field in fields {
                    bytes.extend(field.serialize()?);
                }
                Some(bytes)
            }
            MiriValue::Enum { .. } | MiriValue::FuncPtr(_) => None,
        }
    }

    /// Element count (for arrays/tuples serialized as flat data).
    pub fn elem_count(&self) -> usize {
        match self {
            MiriValue::Array(elems) => elems.len(),
            MiriValue::Tuple(fields) => fields.len(),
            _ => 1,
        }
    }

    /// Type prefix for codegen dispatch.
    pub fn type_prefix(&self) -> &'static str {
        match self {
            MiriValue::Unit => "Unit",
            MiriValue::Bool(_) => "Bool",
            MiriValue::I8(_) => "I8",
            MiriValue::I16(_) => "I16",
            MiriValue::I32(_) => "I32",
            MiriValue::I64(_) => "I64",
            MiriValue::U8(_) => "U8",
            MiriValue::U16(_) => "U16",
            MiriValue::U32(_) => "U32",
            MiriValue::U64(_) => "U64",
            MiriValue::F32(_) => "F32",
            MiriValue::F64(_) => "F64",
            MiriValue::Char(_) => "Char",
            MiriValue::String(_) => "String",
            MiriValue::Array(_) => "Array",
            MiriValue::Tuple(_) => "Tuple",
            MiriValue::Struct { .. } => "Struct",
            MiriValue::Enum { .. } => "Enum",
            MiriValue::FuncPtr(_) => "FuncPtr",
        }
    }
}

// ── Errors ──────────────────────────────────────────────────────────

/// Errors during MIR interpretation.
#[derive(Debug, Clone)]
pub enum MiriError {
    UninitializedLocal(LocalId),
    DivisionByZero,
    BranchLimitExceeded(u64),
    UnsupportedOperation(String),
    StackOverflow,
    Unreachable,
}

impl MiriError {
    /// Shorthand for type mismatch errors.
    pub fn type_mismatch(expected: &str, got: &MiriValue) -> Self {
        MiriError::UnsupportedOperation(format!("expected {expected}, got {got:?}"))
    }
}

impl fmt::Display for MiriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MiriError::UninitializedLocal(id) => write!(f, "use of uninitialized local _{}", id.0),
            MiriError::DivisionByZero => write!(f, "division by zero"),
            MiriError::BranchLimitExceeded(limit) => {
                write!(f, "compile-time evaluation exceeded branch limit ({limit})")
            }
            MiriError::UnsupportedOperation(msg) => write!(f, "{msg}"),
            MiriError::StackOverflow => write!(f, "compile-time evaluation exceeded stack depth"),
            MiriError::Unreachable => write!(f, "reached unreachable code"),
        }
    }
}

impl std::error::Error for MiriError {}

// ── Engine ──────────────────────────────────────────────────────────

/// MIR interpreter engine.
pub struct MiriEngine {
    /// All available functions (user-defined MIR).
    pub(crate) functions: HashMap<String, MirFunction>,
    /// Struct layouts from monomorphization.
    pub(crate) struct_layouts: Vec<StructLayout>,
    /// Enum layouts from monomorphization.
    #[allow(dead_code)]
    pub(crate) enum_layouts: Vec<EnumLayout>,
    /// Stdlib dispatch.
    pub(crate) stdlib: Box<dyn StdlibProvider>,
    /// Call stack.
    pub(crate) stack: memory::CallStack,
    /// Backwards branch quota (default 1000 per spec CT35).
    pub(crate) branch_limit: u64,
    /// Current backwards branch count.
    pub(crate) branch_count: u64,
    /// Pre-evaluated comptime globals (available to GlobalRef).
    pub(crate) comptime_globals: HashMap<String, MiriValue>,
    /// Current source location (for error messages).
    pub(crate) current_line: u32,
    pub(crate) current_col: u32,
}

impl MiriEngine {
    /// Create a new engine with the given stdlib provider.
    pub fn new(stdlib: Box<dyn StdlibProvider>) -> Self {
        Self {
            functions: HashMap::new(),
            struct_layouts: Vec::new(),
            enum_layouts: Vec::new(),
            stdlib,
            stack: memory::CallStack::new(),
            branch_limit: 1_000,
            branch_count: 0,
            comptime_globals: HashMap::new(),
            current_line: 0,
            current_col: 0,
        }
    }

    /// Register a MIR function.
    pub fn register_function(&mut self, func: MirFunction) {
        self.functions.insert(func.name.clone(), func);
    }

    /// Set struct layouts from monomorphization.
    pub fn set_struct_layouts(&mut self, layouts: Vec<StructLayout>) {
        self.struct_layouts = layouts;
    }

    /// Set enum layouts from monomorphization.
    pub fn set_enum_layouts(&mut self, layouts: Vec<EnumLayout>) {
        self.enum_layouts = layouts;
    }

    /// Set the backwards branch limit.
    pub fn set_branch_limit(&mut self, limit: u64) {
        self.branch_limit = limit;
    }

    /// Register a pre-evaluated comptime global.
    pub fn register_global(&mut self, name: String, value: MiriValue) {
        self.comptime_globals.insert(name, value);
    }

    /// Execute a function by name. Resets branch counter.
    pub fn execute(
        &mut self,
        func_name: &str,
        args: Vec<MiriValue>,
    ) -> Result<MiriValue, MiriError> {
        self.branch_count = 0;
        self.call_function(func_name, args)
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_mir::*;

    /// Helper: build a simple MIR function with one block.
    fn make_func(
        name: &str,
        params: Vec<MirLocal>,
        ret_ty: MirType,
        locals: Vec<MirLocal>,
        blocks: Vec<MirBlock>,
    ) -> MirFunction {
        MirFunction {
            name: name.to_string(),
            params,
            ret_ty,
            locals,
            blocks,
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn local(id: u32, ty: MirType) -> MirLocal {
        MirLocal {
            id: LocalId(id),
            name: None,
            ty,
            is_param: false,
        }
    }

    fn param(id: u32, ty: MirType) -> MirLocal {
        MirLocal {
            id: LocalId(id),
            name: None,
            ty,
            is_param: true,
        }
    }

    #[test]
    fn test_return_constant() {
        // func f() -> i64 { return 42 }
        let func = make_func(
            "f",
            vec![],
            MirType::I64,
            vec![],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Constant(MirConst::Int(42))),
                }),
            }],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(func);
        let result = engine.execute("f", vec![]).unwrap();
        assert!(matches!(result, MiriValue::I64(42)));
    }

    #[test]
    fn test_arithmetic() {
        // func add(a: i64, b: i64) -> i64 { return a + b }
        let func = make_func(
            "add",
            vec![param(0, MirType::I64), param(1, MirType::I64)],
            MirType::I64,
            vec![
                param(0, MirType::I64),
                param(1, MirType::I64),
                local(2, MirType::I64),
            ],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                    dst: LocalId(2),
                    rvalue: MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: MirOperand::Local(LocalId(0)),
                        right: MirOperand::Local(LocalId(1)),
                    },
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(LocalId(2))),
                }),
            }],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(func);
        let result = engine.execute("add", vec![MiriValue::I64(10), MiriValue::I64(32)]).unwrap();
        assert!(matches!(result, MiriValue::I64(42)));
    }

    #[test]
    fn test_branch() {
        // func abs(x: i64) -> i64 {
        //   if x < 0: return -x
        //   else: return x
        // }
        let func = make_func(
            "abs",
            vec![param(0, MirType::I64)],
            MirType::I64,
            vec![
                param(0, MirType::I64),
                local(1, MirType::Bool),  // cond
                local(2, MirType::I64),   // result
            ],
            vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                        dst: LocalId(1),
                        rvalue: MirRValue::BinaryOp {
                            op: BinOp::Lt,
                            left: MirOperand::Local(LocalId(0)),
                            right: MirOperand::Constant(MirConst::Int(0)),
                        },
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(LocalId(1)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                        dst: LocalId(2),
                        rvalue: MirRValue::UnaryOp {
                            op: UnaryOp::Neg,
                            operand: MirOperand::Local(LocalId(0)),
                        },
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(LocalId(2))),
                    }),
                },
                MirBlock {
                    id: BlockId(2),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(LocalId(0))),
                    }),
                },
            ],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(func);

        let r1 = engine.execute("abs", vec![MiriValue::I64(-5)]).unwrap();
        assert!(matches!(r1, MiriValue::I64(5)));

        let r2 = engine.execute("abs", vec![MiriValue::I64(7)]).unwrap();
        assert!(matches!(r2, MiriValue::I64(7)));
    }

    #[test]
    fn test_loop_with_branch_limit() {
        // func infinite() -> i64 {
        //   bb0: goto bb0  (infinite loop)
        // }
        let func = make_func(
            "infinite",
            vec![],
            MirType::I64,
            vec![],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: BlockId(0),
                }),
            }],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.set_branch_limit(10);
        engine.register_function(func);

        let result = engine.execute("infinite", vec![]);
        assert!(matches!(result, Err(MiriError::BranchLimitExceeded(10))));
    }

    #[test]
    fn test_function_call() {
        // func double(x: i64) -> i64 { return x + x }
        // func main() -> i64 { return double(21) }
        let double = make_func(
            "double",
            vec![param(0, MirType::I64)],
            MirType::I64,
            vec![param(0, MirType::I64), local(1, MirType::I64)],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                    dst: LocalId(1),
                    rvalue: MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: MirOperand::Local(LocalId(0)),
                        right: MirOperand::Local(LocalId(0)),
                    },
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(LocalId(1))),
                }),
            }],
        );

        let main = make_func(
            "main",
            vec![],
            MirType::I64,
            vec![local(0, MirType::I64)],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(LocalId(0)),
                    func: FunctionRef::internal("double".to_string()),
                    args: vec![MirOperand::Constant(MirConst::Int(21))],
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(LocalId(0))),
                }),
            }],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(double);
        engine.register_function(main);
        let result = engine.execute("main", vec![]).unwrap();
        assert!(matches!(result, MiriValue::I64(42)));
    }

    #[test]
    fn test_loop_sum() {
        // func sum() -> i64 {
        //   let acc = 0     // local 0
        //   let i = 0       // local 1
        //   let cond        // local 2
        //   bb0: acc = 0, i = 0, goto bb1
        //   bb1: cond = i < 10, branch cond bb2 bb3
        //   bb2: acc = acc + i, i = i + 1, goto bb1
        //   bb3: return acc
        // }
        let func = make_func(
            "sum",
            vec![],
            MirType::I64,
            vec![
                local(0, MirType::I64),
                local(1, MirType::I64),
                local(2, MirType::Bool),
            ],
            vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: LocalId(0),
                            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                        }),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: LocalId(1),
                            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto {
                        target: BlockId(1),
                    }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                        dst: LocalId(2),
                        rvalue: MirRValue::BinaryOp {
                            op: BinOp::Lt,
                            left: MirOperand::Local(LocalId(1)),
                            right: MirOperand::Constant(MirConst::Int(10)),
                        },
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(LocalId(2)),
                        then_block: BlockId(2),
                        else_block: BlockId(3),
                    }),
                },
                MirBlock {
                    id: BlockId(2),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: LocalId(0),
                            rvalue: MirRValue::BinaryOp {
                                op: BinOp::Add,
                                left: MirOperand::Local(LocalId(0)),
                                right: MirOperand::Local(LocalId(1)),
                            },
                        }),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: LocalId(1),
                            rvalue: MirRValue::BinaryOp {
                                op: BinOp::Add,
                                left: MirOperand::Local(LocalId(1)),
                                right: MirOperand::Constant(MirConst::Int(1)),
                            },
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto {
                        target: BlockId(1),
                    }),
                },
                MirBlock {
                    id: BlockId(3),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(LocalId(0))),
                    }),
                },
            ],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(func);
        let result = engine.execute("sum", vec![]).unwrap();
        // 0+1+2+...+9 = 45
        assert!(matches!(result, MiriValue::I64(45)));
    }

    #[test]
    fn test_division_by_zero() {
        let func = make_func(
            "div",
            vec![],
            MirType::I64,
            vec![local(0, MirType::I64)],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                    dst: LocalId(0),
                    rvalue: MirRValue::BinaryOp {
                        op: BinOp::Div,
                        left: MirOperand::Constant(MirConst::Int(10)),
                        right: MirOperand::Constant(MirConst::Int(0)),
                    },
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(LocalId(0))),
                }),
            }],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(func);
        let result = engine.execute("div", vec![]);
        assert!(matches!(result, Err(MiriError::DivisionByZero)));
    }

    #[test]
    fn test_string_constant() {
        let func = make_func(
            "greet",
            vec![],
            MirType::String,
            vec![],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Constant(MirConst::String("hello".to_string()))),
                }),
            }],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(func);
        let result = engine.execute("greet", vec![]).unwrap();
        assert!(matches!(result, MiriValue::String(s) if s == "hello"));
    }

    #[test]
    fn test_serialization() {
        assert_eq!(MiriValue::I64(42).serialize(), Some(42i64.to_le_bytes().to_vec()));
        assert_eq!(MiriValue::Bool(true).serialize(), Some(vec![1]));
        assert_eq!(MiriValue::F64(3.14).serialize(), Some(3.14f64.to_le_bytes().to_vec()));

        let arr = MiriValue::Array(vec![MiriValue::I64(1), MiriValue::I64(2), MiriValue::I64(3)]);
        let bytes = arr.serialize().unwrap();
        assert_eq!(bytes.len(), 24); // 3 × 8 bytes
    }

    #[test]
    fn test_switch() {
        // func classify(x: i64) -> i64 {
        //   switch x { 0 => return 10, 1 => return 20, default => return 30 }
        // }
        let func = make_func(
            "classify",
            vec![param(0, MirType::I64)],
            MirType::I64,
            vec![param(0, MirType::I64)],
            vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Switch {
                        value: MirOperand::Local(LocalId(0)),
                        cases: vec![(0, BlockId(1)), (1, BlockId(2))],
                        default: BlockId(3),
                    }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Constant(MirConst::Int(10))),
                    }),
                },
                MirBlock {
                    id: BlockId(2),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Constant(MirConst::Int(20))),
                    }),
                },
                MirBlock {
                    id: BlockId(3),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Constant(MirConst::Int(30))),
                    }),
                },
            ],
        );

        let mut engine = MiriEngine::new(Box::new(PureStdlib));
        engine.register_function(func);

        assert!(matches!(engine.execute("classify", vec![MiriValue::I64(0)]).unwrap(), MiriValue::I64(10)));
        assert!(matches!(engine.execute("classify", vec![MiriValue::I64(1)]).unwrap(), MiriValue::I64(20)));
        assert!(matches!(engine.execute("classify", vec![MiriValue::I64(99)]).unwrap(), MiriValue::I64(30)));
    }
}
