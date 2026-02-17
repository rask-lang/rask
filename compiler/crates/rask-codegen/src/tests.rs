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

    // ═══════════════════════════════════════════════════════════
    // Struct field access (Field rvalue)
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_struct_field_access() {
        // struct Point { x: i32, y: i32 }
        // func get_y(p: Point) -> i32 { return p.y }
        //
        // MIR: p is a pointer to 8 bytes on stack (Struct(0))
        //   _1 = Field { base: _0, field_index: 1 }
        //   return _1
        use rask_mir::MirType;

        let struct_layout_id = rask_mir::MirType::Struct(rask_mir::StructLayoutId(0));

        let mir = MirFunction {
            name: "get_y".to_string(),
            params: vec![local(0, "p", struct_layout_id.clone(), true)],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "p", struct_layout_id, true),
                temp(1, MirType::I32),
            ],
            blocks: vec![
                block(0, vec![
                    assign(1, MirRValue::Field { base: local_op(0), field_index: 1 }),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mono = mono_with_point_struct();
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&mono, &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_struct_store_and_field() {
        // Construct a struct on the stack, then read a field.
        //
        //   _0: Struct(0)  — stack allocated
        //   store _0 + 0, 10   — x = 10
        //   store _0 + 4, 20   — y = 20
        //   _1 = Field { base: _0, field_index: 1 }  — read y
        //   return _1
        let struct_ty = rask_mir::MirType::Struct(rask_mir::StructLayoutId(0));

        let mir = MirFunction {
            name: "make_point".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![
                temp(0, struct_ty),
                temp(1, MirType::I32),
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::Store { addr: LocalId(0), offset: 0, value: i32_const(10) },
                    MirStmt::Store { addr: LocalId(0), offset: 4, value: i32_const(20) },
                    assign(1, MirRValue::Field { base: local_op(0), field_index: 1 }),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mono = mono_with_point_struct();
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&mono, &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Enum tag extraction (EnumTag rvalue)
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_enum_tag() {
        // enum Result { Ok(i32), Err(i32) }
        // func get_tag(r: Result) -> u8 { return r.tag }
        //
        // MIR: _1 = EnumTag { value: _0 }
        let enum_ty = rask_mir::MirType::Enum(rask_mir::EnumLayoutId(0));

        let mir = MirFunction {
            name: "get_tag".to_string(),
            params: vec![local(0, "r", enum_ty.clone(), true)],
            ret_ty: MirType::U8,
            locals: vec![
                local(0, "r", enum_ty, true),
                temp(1, MirType::U8),
            ],
            blocks: vec![
                block(0, vec![
                    assign(1, MirRValue::EnumTag { value: local_op(0) }),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mono = mono_with_result_enum();
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&mono, &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_enum_field_access() {
        // Extract payload from enum after tag check.
        //
        //   _0: Enum(0) — result param
        //   _1 = EnumTag { value: _0 }
        //   branch _1 == 0 → bb1 (Ok), else bb2
        //   bb1: _2 = Field { base: _0, field_index: 0 }  — extract Ok payload
        //        return _2
        //   bb2: return 0
        let enum_ty = rask_mir::MirType::Enum(rask_mir::EnumLayoutId(0));

        let mir = MirFunction {
            name: "unwrap_ok".to_string(),
            params: vec![local(0, "r", enum_ty.clone(), true)],
            ret_ty: MirType::I32,
            locals: vec![
                local(0, "r", enum_ty, true),
                temp(1, MirType::U8),  // tag
                temp(2, MirType::I32), // extracted payload
                temp(3, MirType::Bool), // comparison
            ],
            blocks: vec![
                // bb0: extract tag and branch
                block(0, vec![
                    assign(1, MirRValue::EnumTag { value: local_op(0) }),
                    assign(3, MirRValue::BinaryOp {
                        op: BinOp::Eq,
                        left: local_op(1),
                        right: MirOperand::Constant(MirConst::Int(0)),
                    }),
                ], branch(local_op(3), 1, 2)),
                // bb1: Ok case
                block(1, vec![
                    assign(2, MirRValue::Field { base: local_op(0), field_index: 0 }),
                ], ret(Some(local_op(2)))),
                // bb2: Err case
                block(2, vec![], ret(Some(i32_const(0)))),
            ],
            entry_block: BlockId(0),
        };

        let mono = mono_with_result_enum();
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&mono, &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Pointer ref/deref
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_ref_aggregate() {
        // Ref on an aggregate returns the pointer (no extra work).
        //   _0: Struct(0) — stack allocated
        //   _1: Ptr = Ref(_0)
        //   return _1
        let struct_ty = rask_mir::MirType::Struct(rask_mir::StructLayoutId(0));

        let mir = MirFunction {
            name: "ref_struct".to_string(),
            params: vec![],
            ret_ty: MirType::Ptr,
            locals: vec![
                temp(0, struct_ty),
                temp(1, MirType::Ptr),
            ],
            blocks: vec![
                block(0, vec![
                    assign(1, MirRValue::Ref(LocalId(0))),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mono = mono_with_point_struct();
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&mono, &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_ref_scalar() {
        // Ref on a scalar spills to a stack slot and returns the address.
        //   _0: i32 = 42
        //   _1: Ptr = Ref(_0)
        //   return _1
        let mir = MirFunction {
            name: "ref_scalar".to_string(),
            params: vec![],
            ret_ty: MirType::Ptr,
            locals: vec![
                temp(0, MirType::I32),
                temp(1, MirType::Ptr),
            ],
            blocks: vec![
                block(0, vec![
                    assign(0, MirRValue::Use(i32_const(42))),
                    assign(1, MirRValue::Ref(LocalId(0))),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_deref() {
        // Deref: load from a pointer.
        //   _0: i32 = 42
        //   _1: Ptr = Ref(_0)
        //   _2: i32 = Deref(_1)
        //   return _2
        let mir = MirFunction {
            name: "deref_test".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![
                temp(0, MirType::I32),
                temp(1, MirType::Ptr),
                temp(2, MirType::I32),
            ],
            blocks: vec![
                block(0, vec![
                    assign(0, MirRValue::Use(i32_const(42))),
                    assign(1, MirRValue::Ref(LocalId(0))),
                    assign(2, MirRValue::Deref(local_op(1))),
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
    // String data section
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_string_constant() {
        // func greet() { rask_print_string("hello") }
        let mir = MirFunction {
            name: "greet".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    call(None, "rask_print_string", vec![
                        MirOperand::Constant(MirConst::String("hello".to_string())),
                    ]),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.register_strings(&[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_string_dedup() {
        // Same string used twice should not cause errors (deduplication).
        let mir = MirFunction {
            name: "greet2".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    call(None, "rask_print_string", vec![
                        MirOperand::Constant(MirConst::String("dup".to_string())),
                    ]),
                    call(None, "rask_print_string", vec![
                        MirOperand::Constant(MirConst::String("dup".to_string())),
                    ]),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.register_strings(&[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_string_in_assign() {
        // _0: String = "hello world"
        // return _0  (as i64 pointer)
        let mir = MirFunction {
            name: "str_val".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::String),
            ],
            blocks: vec![
                block(0, vec![
                    assign(0, MirRValue::Use(
                        MirOperand::Constant(MirConst::String("hello world".to_string())),
                    )),
                ], ret(Some(local_op(0)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.register_strings(&[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Stack allocation for aggregates
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_stack_alloc_struct() {
        // A non-param struct local should get a stack allocation.
        //   _0: Struct(0) — auto-allocated stack slot
        //   store _0 + 0, 1
        //   store _0 + 4, 2
        //   return 0
        let struct_ty = rask_mir::MirType::Struct(rask_mir::StructLayoutId(0));

        let mir = MirFunction {
            name: "alloc_struct".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![
                temp(0, struct_ty),
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::Store { addr: LocalId(0), offset: 0, value: i32_const(1) },
                    MirStmt::Store { addr: LocalId(0), offset: 4, value: i32_const(2) },
                ], ret(Some(i32_const(0)))),
            ],
            entry_block: BlockId(0),
        };

        let mono = mono_with_point_struct();
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&mono, &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_stack_alloc_enum() {
        // A non-param enum local should get a stack allocation.
        //   _0: Enum(0) — auto-allocated stack slot
        //   store _0 + 0, 0  — tag = Ok
        //   store _0 + 4, 42 — payload = 42
        //   _1 = EnumTag { value: _0 }
        //   return _1
        let enum_ty = rask_mir::MirType::Enum(rask_mir::EnumLayoutId(0));

        let mir = MirFunction {
            name: "alloc_enum".to_string(),
            params: vec![],
            ret_ty: MirType::U8,
            locals: vec![
                temp(0, enum_ty),
                temp(1, MirType::U8),
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::Store { addr: LocalId(0), offset: 0, value: MirOperand::Constant(MirConst::Int(0)) },
                    MirStmt::Store { addr: LocalId(0), offset: 4, value: i32_const(42) },
                    assign(1, MirRValue::EnumTag { value: local_op(0) }),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mono = mono_with_result_enum();
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&mono, &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // I/O runtime function declarations
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_io_functions_declared() {
        // Verify that I/O runtime functions can be called from generated code.
        let mir = MirFunction {
            name: "io_test".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::I64), // fd from open
                temp(1, MirType::I64), // write result
                temp(2, MirType::I64), // close result
            ],
            blocks: vec![
                block(0, vec![
                    // fd = rask_io_open(path, flags, mode)
                    call(Some(0), "rask_io_open", vec![
                        MirOperand::Constant(MirConst::Int(0)), // null path
                        MirOperand::Constant(MirConst::Int(0)), // flags
                        MirOperand::Constant(MirConst::Int(0)), // mode
                    ]),
                    // rask_io_write(fd, buf, len)
                    call(Some(1), "rask_io_write", vec![
                        local_op(0),
                        MirOperand::Constant(MirConst::Int(0)),
                        MirOperand::Constant(MirConst::Int(0)),
                    ]),
                    // rask_io_close(fd)
                    call(Some(2), "rask_io_close", vec![local_op(0)]),
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
    // Resource tracking
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_resource_register_consume() {
        // func f() {
        //   _0 = ResourceRegister("File", scope_depth=1)
        //   ResourceConsume(_0)
        //   ResourceScopeCheck(1)
        // }
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                temp(0, MirType::I64), // resource id
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::ResourceRegister {
                        dst: LocalId(0),
                        type_name: "File".to_string(),
                        scope_depth: 1,
                    },
                    MirStmt::ResourceConsume {
                        resource_id: LocalId(0),
                    },
                    MirStmt::ResourceScopeCheck {
                        scope_depth: 1,
                    },
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = gen_with_stdlib();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Pool checked access
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_pool_checked_access() {
        // func f(pool: i64, handle: i64) -> i64 {
        //   _2 = PoolCheckedAccess { pool: _0, handle: _1 }
        //   return _2
        // }
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![
                local(0, "pool", MirType::I64, true),
                local(1, "handle", MirType::I64, true),
            ],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "pool", MirType::I64, true),
                local(1, "handle", MirType::I64, true),
                temp(2, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::PoolCheckedAccess {
                        dst: LocalId(2),
                        pool: LocalId(0),
                        handle: LocalId(1),
                    },
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = gen_with_stdlib();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Ensure push/pop (no-ops)
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_ensure_push_pop_noop() {
        // EnsurePush/Pop should compile without error (no-ops).
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    MirStmt::EnsurePush { cleanup_block: BlockId(99) },
                    MirStmt::EnsurePop,
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
    // CleanupReturn
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_cleanup_return_empty_chain() {
        // CleanupReturn with empty cleanup_chain = plain return.
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![],
            blocks: vec![
                block(0, vec![], MirTerminator::CleanupReturn {
                    value: Some(i32_const(42)),
                    cleanup_chain: vec![],
                }),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_cleanup_return_with_chain() {
        // CleanupReturn that inlines a cleanup block before returning.
        //
        // bb0: CleanupReturn { value: 99, cleanup_chain: [bb1] }
        // bb1 (cleanup): call rask_print_i64(0)  (side effect)
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![],
            blocks: vec![
                block(0, vec![], MirTerminator::CleanupReturn {
                    value: Some(i32_const(99)),
                    cleanup_chain: vec![BlockId(1)],
                }),
                block(1, vec![
                    call(None, "rask_print_i64", vec![i32_const(0)]),
                ], ret(None)), // terminator ignored for cleanup blocks
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Stdlib dispatch
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_stdlib_vec_push() {
        // Calling stdlib Vec methods through dispatch.
        // _0 = Vec_new()
        // Vec_push(_0, 42)
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                temp(0, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    call(Some(0), "Vec_new", vec![]),
                    call(None, "Vec_push", vec![local_op(0), i32_const(42)]),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = gen_with_stdlib();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_stdlib_vec_len() {
        // _0 = Vec_new()
        // _1 = Vec_len(_0)
        // return _1
        let mir = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::I64),
                temp(1, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    call(Some(0), "Vec_new", vec![]),
                    call(Some(1), "Vec_len", vec![local_op(0)]),
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = gen_with_stdlib();
        gen.declare_functions(&dummy_mono(), &[mir.clone()]).unwrap();
        gen.gen_function(&mir).unwrap();
    }

    #[test]
    fn codegen_stdlib_user_overrides_stdlib() {
        // User-defined "push" function should shadow stdlib "push".
        let push_fn = MirFunction {
            name: "push".to_string(),
            params: vec![local(0, "x", MirType::I32, true)],
            ret_ty: MirType::Void,
            locals: vec![local(0, "x", MirType::I32, true)],
            blocks: vec![block(0, vec![], ret(None))],
            entry_block: BlockId(0),
        };

        let caller_fn = MirFunction {
            name: "caller".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![
                block(0, vec![
                    call(None, "push", vec![i32_const(1)]),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = gen_with_stdlib();
        gen.declare_functions(&dummy_mono(), &[push_fn.clone(), caller_fn.clone()]).unwrap();
        gen.gen_function(&push_fn).unwrap();
        gen.gen_function(&caller_fn).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Closure environment layout (unit test)
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn closure_env_layout_sizing() {
        use crate::closures::ClosureEnvLayout;

        let mut layout = ClosureEnvLayout::new();
        assert_eq!(layout.size, 0);

        let off0 = layout.add_capture(LocalId(0), 8);
        assert_eq!(off0, 0);
        assert_eq!(layout.size, 8);

        let off1 = layout.add_capture(LocalId(1), 4);
        assert_eq!(off1, 8); // aligned to 8
        assert_eq!(layout.size, 12);

        let off2 = layout.add_capture(LocalId(2), 8);
        assert_eq!(off2, 16); // aligned up from 12 → 16
        assert_eq!(layout.size, 24);

        assert_eq!(layout.captures.len(), 3);
    }

    // ═══════════════════════════════════════════════════════════
    // Closure heap allocation
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_closure_stack_no_captures() {
        // Non-escaping closure with no captures: stack-allocated.
        //
        //   closure_fn(env: ptr, x: i64) -> i64 { return x }
        //   main() -> i64 {
        //     _0 = ClosureCreate[stack] { func: closure_fn, captures: [] }
        //     _1 = ClosureCall(_0, 42)
        //     return _1
        //   }

        let closure_fn = MirFunction {
            name: "main__closure_0".to_string(),
            params: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
            ],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
            ],
            blocks: vec![
                block(0, vec![], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::Ptr),  // closure
                temp(1, MirType::I64),  // call result
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![],
                        heap: false,
                    },
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![MirOperand::Constant(MirConst::Int(42))],
                    },
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[closure_fn.clone(), main_fn.clone()]).unwrap();
        gen.gen_function(&closure_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    #[test]
    fn codegen_closure_create_with_captures() {
        // Closure capturing a variable: heap-allocates func_ptr + capture.
        //
        //   closure_fn(env: ptr) -> i64 {
        //     _2 = LoadCapture(env, offset=0)  // load captured 'factor'
        //     return _2
        //   }
        //   main() -> i64 {
        //     _0: i64 = 10          // factor
        //     _1 = ClosureCreate { func: closure_fn, captures: [_0] }
        //     _2 = ClosureCall(_1)
        //     return _2
        //   }
        use rask_mir::ClosureCapture;

        let closure_fn = MirFunction {
            name: "main__closure_0".to_string(),
            params: vec![
                local(0, "__env", MirType::Ptr, true),
            ],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                temp(1, MirType::I64), // loaded capture
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::LoadCapture {
                        dst: LocalId(1),
                        env_ptr: LocalId(0),
                        offset: 0,
                    },
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::I64),  // factor
                temp(1, MirType::Ptr),  // closure
                temp(2, MirType::I64),  // call result
            ],
            blocks: vec![
                block(0, vec![
                    assign(0, MirRValue::Use(MirOperand::Constant(MirConst::Int(10)))),
                    MirStmt::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![
                            ClosureCapture {
                                local_id: LocalId(0),
                                offset: 0,
                                size: 8,
                            },
                        ],
                        heap: false,
                    },
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![],
                    },
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[closure_fn.clone(), main_fn.clone()]).unwrap();
        gen.gen_function(&closure_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    #[test]
    fn codegen_closure_returned_from_function() {
        // Closure returned from a function — the key case that required
        // heap allocation. With stack allocation this would be a dangling pointer.
        //
        //   closure_fn(env: ptr) -> i64 {
        //     _1 = LoadCapture(env, offset=0)
        //     return _1
        //   }
        //   make_closure() -> ptr {
        //     _0 = 99
        //     _1 = ClosureCreate { func: closure_fn, captures: [_0] }
        //     return _1           // ← escapes! needs heap allocation
        //   }
        //   main() -> i64 {
        //     _0 = make_closure()
        //     _1 = ClosureCall(_0)
        //     return _1
        //   }
        use rask_mir::ClosureCapture;

        let closure_fn = MirFunction {
            name: "make_closure__closure_0".to_string(),
            params: vec![local(0, "__env", MirType::Ptr, true)],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                temp(1, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::LoadCapture {
                        dst: LocalId(1),
                        env_ptr: LocalId(0),
                        offset: 0,
                    },
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let make_fn = MirFunction {
            name: "make_closure".to_string(),
            params: vec![],
            ret_ty: MirType::Ptr,
            locals: vec![
                temp(0, MirType::I64),  // captured value
                temp(1, MirType::Ptr),  // closure
            ],
            blocks: vec![
                block(0, vec![
                    assign(0, MirRValue::Use(MirOperand::Constant(MirConst::Int(99)))),
                    MirStmt::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "make_closure__closure_0".to_string(),
                        captures: vec![
                            ClosureCapture { local_id: LocalId(0), offset: 0, size: 8 },
                        ],
                        heap: true,
                    },
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::Ptr),  // closure from make_closure
                temp(1, MirType::I64),  // call result
            ],
            blocks: vec![
                block(0, vec![
                    call(Some(0), "make_closure", vec![]),
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    },
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(
            &dummy_mono(),
            &[closure_fn.clone(), make_fn.clone(), main_fn.clone()],
        ).unwrap();
        gen.gen_function(&closure_fn).unwrap();
        gen.gen_function(&make_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    #[test]
    fn codegen_closure_drop() {
        // Heap closure created and dropped before returning a different value.
        //
        //   closure_fn(env: ptr) -> i64 { return 1 }
        //   main() -> i64 {
        //     _0 = 99
        //     _1 = closure[heap](closure_fn, [_0])
        //     closure_drop(_1)
        //     return 0
        //   }
        use rask_mir::ClosureCapture;

        let closure_fn = MirFunction {
            name: "main__closure_0".to_string(),
            params: vec![
                local(0, "__env", MirType::Ptr, true),
            ],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
            ],
            blocks: vec![
                block(0, vec![], ret(Some(MirOperand::Constant(MirConst::Int(1))))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::I64),  // captured val
                temp(1, MirType::Ptr),  // closure
            ],
            blocks: vec![
                block(0, vec![
                    assign(0, MirRValue::Use(MirOperand::Constant(MirConst::Int(99)))),
                    MirStmt::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![
                            ClosureCapture { local_id: LocalId(0), offset: 0, size: 8 },
                        ],
                        heap: true,
                    },
                    MirStmt::ClosureDrop {
                        closure: LocalId(1),
                    },
                ], ret(Some(MirOperand::Constant(MirConst::Int(0))))),
            ],
            entry_block: BlockId(0),
        };

        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &[closure_fn.clone(), main_fn.clone()]).unwrap();
        gen.gen_function(&closure_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    // ═══════════════════════════════════════════════════════════
    // Closure edge cases: nested, loops, match arms
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn codegen_nested_closure_captures() {
        // Inner closure captures a variable that the outer closure captured.
        //
        //   inner(env) -> i64 {
        //     _1 = LoadCapture(env, 0)    // inner reads outer's captured 'x'
        //     return _1
        //   }
        //   outer(env) -> i64 {
        //     _1 = LoadCapture(env, 0)    // outer loads captured 'x'
        //     _2 = ClosureCreate[stack](inner, captures=[_1])
        //     _3 = ClosureCall(_2)
        //     return _3
        //   }
        //   main() -> i64 {
        //     _0 = 42
        //     _1 = ClosureCreate[stack](outer, captures=[_0])
        //     _2 = ClosureCall(_1)
        //     return _2
        //   }
        use rask_mir::ClosureCapture;

        let inner_fn = MirFunction {
            name: "main__closure_1".to_string(),
            params: vec![local(0, "__env", MirType::Ptr, true)],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                temp(1, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::LoadCapture { dst: LocalId(1), env_ptr: LocalId(0), offset: 0 },
                ], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let outer_fn = MirFunction {
            name: "main__closure_0".to_string(),
            params: vec![local(0, "__env", MirType::Ptr, true)],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                temp(1, MirType::I64),  // loaded capture
                temp(2, MirType::Ptr),  // inner closure
                temp(3, MirType::I64),  // call result
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::LoadCapture { dst: LocalId(1), env_ptr: LocalId(0), offset: 0 },
                    MirStmt::ClosureCreate {
                        dst: LocalId(2),
                        func_name: "main__closure_1".to_string(),
                        captures: vec![
                            ClosureCapture { local_id: LocalId(1), offset: 0, size: 8 },
                        ],
                        heap: false,
                    },
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(3)),
                        closure: LocalId(2),
                        args: vec![],
                    },
                ], ret(Some(local_op(3)))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::I64),  // x
                temp(1, MirType::Ptr),  // outer closure
                temp(2, MirType::I64),  // result
            ],
            blocks: vec![
                block(0, vec![
                    assign(0, MirRValue::Use(MirOperand::Constant(MirConst::Int(42)))),
                    MirStmt::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![
                            ClosureCapture { local_id: LocalId(0), offset: 0, size: 8 },
                        ],
                        heap: false,
                    },
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![],
                    },
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let all_fns = [inner_fn.clone(), outer_fn.clone(), main_fn.clone()];
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &all_fns).unwrap();
        gen.gen_function(&inner_fn).unwrap();
        gen.gen_function(&outer_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    #[test]
    fn codegen_closure_in_loop_body() {
        // Closure created inside a loop body (new allocation per iteration).
        //
        //   closure_fn(env, x: i64) -> i64 { return x }
        //   main() -> i64 {
        //     block0: _0 = 0; goto block1
        //     block1: branch(_0 < 3, block2, block3)
        //     block2:
        //       _1 = ClosureCreate[stack](closure_fn, captures=[])
        //       _2 = ClosureCall(_1, _0)
        //       _0 = _0 + 1
        //       goto block1
        //     block3: return _0
        //   }

        let closure_fn = MirFunction {
            name: "main__closure_0".to_string(),
            params: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
            ],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
            ],
            blocks: vec![
                block(0, vec![], ret(Some(local_op(1)))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::I64),  // counter
                temp(1, MirType::Ptr),  // closure
                temp(2, MirType::I64),  // call result
                temp(3, MirType::I64),  // cond
            ],
            blocks: vec![
                // block0: init
                block(0, vec![
                    assign(0, MirRValue::Use(MirOperand::Constant(MirConst::Int(0)))),
                ], goto(1)),
                // block1: loop condition
                block(1, vec![
                    assign(3, MirRValue::BinaryOp {
                        op: BinOp::Lt,
                        left: local_op(0),
                        right: MirOperand::Constant(MirConst::Int(3)),
                    }),
                ], branch(local_op(3), 2, 3)),
                // block2: loop body — closure per iteration
                block(2, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![],
                        heap: false,
                    },
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![local_op(0)],
                    },
                    assign(0, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(0),
                        right: MirOperand::Constant(MirConst::Int(1)),
                    }),
                ], goto(1)),
                // block3: exit
                block(3, vec![], ret(Some(local_op(0)))),
            ],
            entry_block: BlockId(0),
        };

        let all_fns = [closure_fn.clone(), main_fn.clone()];
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &all_fns).unwrap();
        gen.gen_function(&closure_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    #[test]
    fn codegen_closure_in_match_arm() {
        // Closures created in different match arm blocks.
        //
        //   add_fn(env, x: i64) -> i64 { return x + 1 }
        //   sub_fn(env, x: i64) -> i64 { return x - 1 }
        //   main(flag: i64) -> i64 {
        //     block0: branch(flag, block1, block2)
        //     block1: _1 = ClosureCreate(add_fn); goto block3
        //     block2: _1 = ClosureCreate(sub_fn); goto block3
        //     block3: _2 = ClosureCall(_1, 10); return _2
        //   }

        let add_fn = MirFunction {
            name: "main__closure_0".to_string(),
            params: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
            ],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
                temp(2, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    assign(2, MirRValue::BinaryOp {
                        op: BinOp::Add,
                        left: local_op(1),
                        right: MirOperand::Constant(MirConst::Int(1)),
                    }),
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let sub_fn = MirFunction {
            name: "main__closure_1".to_string(),
            params: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
            ],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "__env", MirType::Ptr, true),
                local(1, "x", MirType::I64, true),
                temp(2, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![
                    assign(2, MirRValue::BinaryOp {
                        op: BinOp::Sub,
                        left: local_op(1),
                        right: MirOperand::Constant(MirConst::Int(1)),
                    }),
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let main_fn = MirFunction {
            name: "main".to_string(),
            params: vec![local(0, "flag", MirType::I64, true)],
            ret_ty: MirType::I64,
            locals: vec![
                local(0, "flag", MirType::I64, true),
                temp(1, MirType::Ptr),  // closure (assigned in both arms)
                temp(2, MirType::I64),  // call result
            ],
            blocks: vec![
                // block0: dispatch
                block(0, vec![], branch(local_op(0), 1, 2)),
                // block1: arm 1 — add closure
                block(1, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![],
                        heap: false,
                    },
                ], goto(3)),
                // block2: arm 2 — sub closure
                block(2, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "main__closure_1".to_string(),
                        captures: vec![],
                        heap: false,
                    },
                ], goto(3)),
                // block3: merge — call whichever closure was created
                block(3, vec![
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![MirOperand::Constant(MirConst::Int(10))],
                    },
                ], ret(Some(local_op(2)))),
            ],
            entry_block: BlockId(0),
        };

        let all_fns = [add_fn.clone(), sub_fn.clone(), main_fn.clone()];
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_functions(&dummy_mono(), &all_fns).unwrap();
        gen.gen_function(&add_fn).unwrap();
        gen.gen_function(&sub_fn).unwrap();
        gen.gen_function(&main_fn).unwrap();
    }

    // ── Helpers ──────────────────────────────────────────────────

    /// CodeGenerator with runtime + stdlib declared (for tests needing stdlib).
    fn gen_with_stdlib() -> CodeGenerator {
        let mut gen = CodeGenerator::new().unwrap();
        gen.declare_runtime_functions().unwrap();
        gen.declare_stdlib_functions().unwrap();
        gen
    }

    fn dummy_mono() -> rask_mono::MonoProgram {
        rask_mono::MonoProgram {
            functions: vec![],
            struct_layouts: vec![],
            enum_layouts: vec![],
        }
    }

    /// MonoProgram with a Point { x: i32, y: i32 } struct at index 0.
    fn mono_with_point_struct() -> rask_mono::MonoProgram {
        rask_mono::MonoProgram {
            functions: vec![],
            struct_layouts: vec![
                rask_mono::StructLayout {
                    name: "Point".to_string(),
                    size: 8,
                    align: 4,
                    fields: vec![
                        rask_mono::FieldLayout {
                            name: "x".to_string(),
                            ty: rask_types::Type::I32,
                            offset: 0,
                            size: 4,
                            align: 4,
                        },
                        rask_mono::FieldLayout {
                            name: "y".to_string(),
                            ty: rask_types::Type::I32,
                            offset: 4,
                            size: 4,
                            align: 4,
                        },
                    ],
                },
            ],
            enum_layouts: vec![],
        }
    }

    /// MonoProgram with Result { Ok(i32), Err(i32) } enum at index 0.
    fn mono_with_result_enum() -> rask_mono::MonoProgram {
        rask_mono::MonoProgram {
            functions: vec![],
            struct_layouts: vec![],
            enum_layouts: vec![
                rask_mono::EnumLayout {
                    name: "Result".to_string(),
                    size: 8,
                    align: 4,
                    tag_ty: rask_types::Type::U8,
                    tag_offset: 0,
                    variants: vec![
                        rask_mono::VariantLayout {
                            name: "Ok".to_string(),
                            tag: 0,
                            payload_offset: 4,
                            payload_size: 4,
                            fields: vec![
                                rask_mono::FieldLayout {
                                    name: "value".to_string(),
                                    ty: rask_types::Type::I32,
                                    offset: 0,
                                    size: 4,
                                    align: 4,
                                },
                            ],
                        },
                        rask_mono::VariantLayout {
                            name: "Err".to_string(),
                            tag: 1,
                            payload_offset: 4,
                            payload_size: 4,
                            fields: vec![
                                rask_mono::FieldLayout {
                                    name: "error".to_string(),
                                    ty: rask_types::Type::I32,
                                    offset: 0,
                                    size: 4,
                                    align: 4,
                                },
                            ],
                        },
                    ],
                },
            ],
        }
    }
}
