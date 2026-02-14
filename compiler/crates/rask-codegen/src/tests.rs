// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Codegen tests — verify MIR lowers to valid Cranelift IR and produces object files.

#[cfg(test)]
mod tests {
    use rask_mir::{
        BlockId, FunctionRef, LocalId, MirConst, MirFunction, MirLocal, MirBlock,
        MirOperand, MirRValue, MirStmt, MirTerminator, MirType, BinOp,
    };
    use crate::CodeGenerator;

    // ── MIR construction helpers ────────────────────────────────

    fn local(id: u32, name: &str, ty: MirType, is_param: bool) -> MirLocal {
        MirLocal { id: LocalId(id), name: Some(name.to_string()), ty, is_param }
    }

    fn temp(id: u32, ty: MirType) -> MirLocal {
        MirLocal { id: LocalId(id), name: None, ty, is_param: false }
    }

    fn block(id: u32, stmts: Vec<MirStmt>, term: MirTerminator) -> MirBlock {
        MirBlock { id: BlockId(id), statements: stmts, terminator: term }
    }

    fn i32_const(n: i64) -> MirOperand {
        MirOperand::Constant(MirConst::Int(n))
    }

    fn local_op(id: u32) -> MirOperand {
        MirOperand::Local(LocalId(id))
    }

    fn assign(dst: u32, rvalue: MirRValue) -> MirStmt {
        MirStmt::Assign { dst: LocalId(dst), rvalue }
    }

    fn call(dst: Option<u32>, name: &str, args: Vec<MirOperand>) -> MirStmt {
        MirStmt::Call {
            dst: dst.map(LocalId),
            func: FunctionRef { name: name.to_string() },
            args,
        }
    }

    fn ret(val: Option<MirOperand>) -> MirTerminator {
        MirTerminator::Return { value: val }
    }

    fn goto(target: u32) -> MirTerminator {
        MirTerminator::Goto { target: BlockId(target) }
    }

    fn branch(cond: MirOperand, then_b: u32, else_b: u32) -> MirTerminator {
        MirTerminator::Branch {
            cond,
            then_block: BlockId(then_b),
            else_block: BlockId(else_b),
        }
    }

    // ═══════════════════════════════════════════════════════════
    // Basic functions
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_return_constant() {
        // func f() -> i32 { return 42 }
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![],
            blocks: vec![
                block(0, vec![], ret(Some(i32_const(42)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_void_return() {
        // func f() { return }
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_params_and_add() {
        // func add(a: i32, b: i32) -> i32 { return a + b }
        let mir = MirFunction {
            name: "add".to_string(),
            params: vec![
                local(0, "a", MirType::I32, true),
                local(1, "b", MirType::I32, true),
            ],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "a", MirType::I32, true),
                local(1, "b", MirType::I32, true),
                temp(2, MirType::I32),
            ],
            blocks: vec![
                block(0, vec![
                    assign(2, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(0),
                        right: local_op(1),
                    }),
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Function calls
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_function_call() {
        // func add(a: i32, b: i32) -> i32 { ... }
        // func main() -> i32 { return add(1, 2) }
        let add_fn = MirFunction {
            name: "add".to_string(),
            params: vec![
                local(0, "a", MirType::I32, true),
                local(1, "b", MirType::I32, true),
            ],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "a", MirType::I32, true),
                local(1, "b", MirType::I32, true),
                temp(2, MirType::I32),
            ],
            blocks: vec![
                block(0, vec![
                    assign(2, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(0),
                        right: local_op(1),
                    }),
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![
                temp(0, MirType::I32), // call result
            ],
            blocks: vec![
                block(0, vec![
                    call(Some(0), "add", vec![i32_const(1), i32_const(2)]),
                ], ret(Some(local_op(0)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[add_fn.clone(), main_fn.clone()]).unwrap();
        gen.gen_function(&add_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    #[test]
    fn codegen_void_call() {
        // func noop() { }
        // func main() { noop(); return }
        let noop_fn = MirFunction {
            name: "noop".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![block(0, vec![], ret(None))],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "caller".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    call(None, "noop", vec![]),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[noop_fn.clone(), main_fn.clone()]).unwrap();
        gen.gen_function(&noop_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    #[test]
    fn codegen_recursive_call() {
        // func countdown(n: i32) -> i32 {
        //   if n <= 0 { return 0 }
        //   return countdown(n - 1)
        // }
        let mir = MirFunction {
            name: "countdown".to_string(),
            params: vec![local(0, "n", MirType::I32, true)],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "n", MirType::I32, true),
                temp(1, MirType::Bool),  // comparison result
                temp(2, MirType::I32),   // n - 1
                temp(3, MirType::I32),   // recursive result
            ],
            blocks: vec![
                // bb0: check n <= 0
                block(0, vec![
                    assign(1, MirRValue::BinaryOp {
                        op: BinOp::Le,
                        left: local_op(0),
                        right: i32_const(0),
                    }),
                ], branch(local_op(1), 1, 2)),
                // bb1: base case
                block(1, vec![], ret(Some(i32_const(0)))),
                // bb2: recursive case
                block(2, vec![
                    assign(2, MirRValue::BinaryOp {
                        op: BinOp::Sub,
                        left: local_op(0),
                        right: i32_const(1),
                    }),
                    call(Some(3), "countdown", vec![local_op(2)]),
                ], ret(Some(local_op(3)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Loops (while via Goto + Branch)
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_while_loop() {
        // func sum_to_n(n: i32) -> i32 {
        //   let sum = 0
        //   let i = 1
        //   while i <= n { sum = sum + i; i = i + 1 }
        //   return sum
        // }
        let mir = MirFunction {
            name: "sum_to_n".to_string(),
            params: vec![local(0, "n", MirType::I32, true)],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "n", MirType::I32, true),
                local(1, "sum", MirType::I32, false),
                local(2, "i", MirType::I32, false),
                temp(3, MirType::Bool),  // condition
                temp(4, MirType::I32),   // sum + i
                temp(5, MirType::I32),   // i + 1
            ],
            blocks: vec![
                // bb0: init
                block(0, vec![
                    assign(1, MirRValue::Use(i32_const(0))),
                    assign(2, MirRValue::Use(i32_const(1))),
                ], goto(1)),
                // bb1: check condition
                block(1, vec![
                    assign(3, MirRValue::BinaryOp {
                        op: BinOp::Le,
                        left: local_op(2),
                        right: local_op(0),
                    }),
                ], branch(local_op(3), 2, 3)),
                // bb2: loop body
                block(2, vec![
                    assign(4, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(1),
                        right: local_op(2),
                    }),
                    assign(1, MirRValue::Use(local_op(4))),
                    assign(5, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(2),
                        right: i32_const(1),
                    }),
                    assign(2, MirRValue::Use(local_op(5))),
                ], goto(1)),
                // bb3: exit
                block(3, vec![], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_loop_with_call() {
        // Tests loops that contain function calls — the combination of
        // Goto/Branch control flow with Call statements.
        //
        // func run(n: i32) -> i32 {
        //   let sum = 0; let i = 0
        //   while i < n { sum = add(sum, i); i = i + 1 }
        //   return sum
        // }
        let add_fn = MirFunction {
            name: "add".to_string(),
            params: vec![
                local(0, "a", MirType::I32, true),
                local(1, "b", MirType::I32, true),
            ],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "a", MirType::I32, true),
                local(1, "b", MirType::I32, true),
                temp(2, MirType::I32),
            ],
            blocks: vec![
                block(0, vec![
                    assign(2, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(0),
                        right: local_op(1),
                    }),
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let run_fn = MirFunction {
            name: "run".to_string(),
            params: vec![local(0, "n", MirType::I32, true)],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "n", MirType::I32, true),
                local(1, "sum", MirType::I32, false),
                local(2, "i", MirType::I32, false),
                temp(3, MirType::Bool),
                temp(4, MirType::I32),  // call result
                temp(5, MirType::I32),  // i + 1
            ],
            blocks: vec![
                block(0, vec![
                    assign(1, MirRValue::Use(i32_const(0))),
                    assign(2, MirRValue::Use(i32_const(0))),
                ], goto(1)),
                block(1, vec![
                    assign(3, MirRValue::BinaryOp {
                        op: BinOp::Lt,
                        left: local_op(2),
                        right: local_op(0),
                    }),
                ], branch(local_op(3), 2, 3)),
                block(2, vec![
                    call(Some(4), "add", vec![local_op(1), local_op(2)]),
                    assign(1, MirRValue::Use(local_op(4))),
                    assign(5, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(2),
                        right: i32_const(1),
                    }),
                    assign(2, MirRValue::Use(local_op(5))),
                ], goto(1)),
                block(3, vec![], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[add_fn.clone(), run_fn.clone()]).unwrap();
        gen.gen_function(&add_fn).unwrap();
        gen.gen_function(&run_fn).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Object emission
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_emit_object() {
        // Full pipeline: create a simple function, generate code, emit .o file
        let mir = MirFunction {
            name: "answer".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![],
            blocks: vec![
                block(0, vec![], ret(Some(i32_const(42)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();

        let path = "/tmp/rask_test_codegen.o";
        gen.emit_object(path).unwrap();

        // Verify the file was created and is non-empty
        let metadata = std::fs::metadata(path).unwrap();
        assert!(metadata.len() > 0);
        std::fs::remove_file(path).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Type conversions (Cast)
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_cast_i32_to_i64() {
        // func f(x: i32) -> i64 { return x as i64 }
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![local(0, "x", MirType::I32, true)],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "x", MirType::I32, true),
                temp(1, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    assign(1, MirRValue::Cast {
                        value: local_op(0),
                        target_ty: MirType::I64,
                    }),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Runtime function calls
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_call_runtime_function() {
        // func main() { rask_print_i64(42) }
        let mir = MirFunction {
            name: "caller".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    call(None, "rask_print_i64", vec![i32_const(42)]),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Error handling
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_unknown_function_errors() {
        // Calling a function that was never declared should produce FunctionNotFound
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    call(None, "nonexistent", vec![]),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        let result = gen.gen_function(&mir);
        assert!(result.is_err());
    }

    #[test]
    fn codegen_source_location_ignored() {
        // SourceLocation statements should not cause errors
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    MirStmt::SourceLocation { line: 1, col: 1 },
                ], ret(Some(i32_const(0)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ── Helpers ──────────────────────────────────────────────────

    fn dummy_mono() -> rask_mono::MonoProgram {
        rask_mono::MonoProgram {
            functions: vec![],
            struct_layouts: vec![],
            enum_layouts: vec![],
        }
    }
}
