// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Rask functions C calls, when a struct crosses by value (struct.c-interop/SR2).
//!
//! A Rask function takes a struct as the address of the caller's copy and
//! hands one back through a destination pointer when it's wider than a word.
//! C does neither: `Rect make(int, int)` returns its eight bytes in a register
//! and `float sum(Vec2)` reads its argument out of a float register. So an
//! exported `extern "C"` function, a closure handed to C, or a context
//! trampoline whose signature holds a struct is compiled twice over: the body
//! keeps Rask's convention under a local name, and the exported name is a thunk
//! that takes C's arguments, lays the structs out in its own frame, calls the
//! body, and hands the answer back the way `rask_mir::c_call` says C expects
//! it. Rask code calling the function calls the body; a pointer taken to it for
//! C is the thunk's.

use cranelift::prelude::*;
use cranelift_codegen::ir::{ArgumentPurpose, Signature, StackSlotData, StackSlotKind};
use cranelift_module::{FuncId, Linkage, Module};
use rask_mir::c_call::{CArg, CRet, CallConv};
use rask_mir::{MirFunction, MirType};

use crate::module::{piece_type, CodeGenerator};
use crate::types::mir_to_cranelift_type;
use crate::{CodegenError, CodegenResult};

/// A Rask function with a C-convention entry of its own.
pub(crate) struct CEntry {
    /// The Rask-convention body.
    pub body: FuncId,
    /// The C-convention thunk under the exported name.
    pub thunk: FuncId,
    pub params: Vec<(MirType, CArg)>,
    pub ret: (MirType, CRet),
}

/// The key a thunk is found under among the function ids. A `FuncAddr` of an
/// `extern "C"` function asks for this first: the address C gets has to be
/// the entry that follows C's rules.
pub(crate) fn c_entry_key(name: &str) -> String {
    format!("{}#c-entry", name)
}

/// The symbol the Rask-convention body takes when the exported name goes to
/// its thunk. Local, so it never clashes with anything C links.
pub(crate) fn c_body_symbol(name: &str) -> String {
    format!("rask_c_body.{}", name)
}

fn slot(b: &mut FunctionBuilder, size: u32) -> Value {
    // Whole words, so a register piece that runs past the struct's last byte
    // still lands inside the slot.
    let size = size.div_ceil(8).max(2) * 8;
    let ss = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 4));
    b.ins().stack_addr(types::I64, ss, 0)
}

impl CodeGenerator {
    /// The calling convention C uses on the target being compiled for.
    pub(crate) fn c_call_conv(&self) -> CallConv {
        let triple = self.module.isa().triple();
        let os = match triple.operating_system {
            target_lexicon::OperatingSystem::Windows => "windows",
            _ => "",
        };
        CallConv::for_target(&triple.architecture.to_string(), os)
    }

    /// How C passes `mir_fn`'s arguments and expects its result, when one of
    /// them is a struct. `None` when Rask's convention and C's already agree.
    pub(crate) fn c_entry_plan(&self, mir_fn: &MirFunction) -> Option<(Vec<(MirType, CArg)>, (MirType, CRet))> {
        if !mir_fn.is_extern_c {
            return None;
        }
        let conv = self.c_call_conv();
        let layout = |ty: &MirType| match ty {
            MirType::Struct(id) => self.struct_layouts.get(id.id as usize),
            _ => None,
        };
        let params: Vec<(MirType, CArg)> = mir_fn
            .params
            .iter()
            .map(|p| {
                let arg = match layout(&p.ty) {
                    Some(l) => rask_mir::c_call::classify_struct(conv, l, &self.struct_layouts),
                    None => CArg::Scalar,
                };
                (p.ty.clone(), arg)
            })
            .collect();
        let ret = match layout(&mir_fn.ret_ty) {
            Some(l) => rask_mir::c_call::classify_struct_return(conv, l, &self.struct_layouts),
            None => CRet::Scalar,
        };
        let any = params.iter().any(|(_, a)| !matches!(a, CArg::Scalar)) || !matches!(ret, CRet::Scalar);
        any.then(|| (params, (mir_fn.ret_ty.clone(), ret)))
    }

    /// The thunk's signature: what a C caller passes and expects.
    pub(crate) fn c_entry_signature(
        &self,
        name: &str,
        params: &[(MirType, CArg)],
        ret: &(MirType, CRet),
    ) -> CodegenResult<Signature> {
        let unsupported = || {
            CodegenError::UnsupportedFeature(format!(
                "`{}` is called from C with a struct, and how one crosses isn't worked out for this target yet",
                name
            ))
        };
        let mut sig = self.module.make_signature();
        if matches!(ret.1, CRet::Indirect) {
            sig.params.push(AbiParam::special(types::I64, ArgumentPurpose::StructReturn));
        }
        for (ty, arg) in params {
            match arg {
                CArg::Scalar => sig.params.push(AbiParam::new(mir_to_cranelift_type(ty)?)),
                CArg::Registers(pieces) => {
                    for p in pieces {
                        sig.params.push(AbiParam::new(piece_type(p.kind)));
                    }
                }
                CArg::StackCopy(size) => {
                    sig.params.push(AbiParam::special(types::I64, ArgumentPurpose::StructArgument(*size)))
                }
                CArg::ByAddress(_) => sig.params.push(AbiParam::new(types::I64)),
                CArg::Unsupported => return Err(unsupported()),
            }
        }
        match &ret.1 {
            CRet::Registers(pieces) => {
                for p in pieces {
                    sig.returns.push(AbiParam::new(piece_type(p.kind)));
                }
            }
            CRet::Indirect => {}
            CRet::Unsupported => return Err(unsupported()),
            CRet::Scalar => {
                if !matches!(ret.0, MirType::Void) {
                    sig.returns.push(AbiParam::new(mir_to_cranelift_type(&ret.0)?));
                }
            }
        }
        Ok(sig)
    }

    /// Declare the body under its local name and the thunk under the exported
    /// one. Answers the body's id, which Rask callers use.
    pub(crate) fn declare_c_entry(
        &mut self,
        mir_fn: &MirFunction,
        export: &str,
        body_sig: &Signature,
        params: Vec<(MirType, CArg)>,
        ret: (MirType, CRet),
    ) -> CodegenResult<FuncId> {
        let body = self
            .module
            .declare_function(&c_body_symbol(&mir_fn.name), Linkage::Local, body_sig)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
        let thunk_sig = self.c_entry_signature(&mir_fn.name, &params, &ret)?;
        let thunk = self
            .module
            .declare_function(export, Linkage::Export, &thunk_sig)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
        self.func_ids.insert(c_entry_key(&mir_fn.name), thunk);
        self.c_entries.push(CEntry { body, thunk, params, ret });
        Ok(body)
    }

    /// Define every thunk declared so far.
    pub(crate) fn gen_c_entries(&mut self) -> CodegenResult<()> {
        let entries = std::mem::take(&mut self.c_entries);
        for e in &entries {
            self.gen_c_entry(e)?;
        }
        Ok(())
    }

    fn gen_c_entry(&mut self, e: &CEntry) -> CodegenResult<()> {
        let sig = self.c_entry_signature("thunk", &e.params, &e.ret)?;
        self.ctx.clear();
        self.ctx.func.signature = sig;
        let body = self.module.declare_func_in_func(e.body, &mut self.ctx.func);
        let mut fbc = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut self.ctx.func, &mut fbc);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        b.seal_block(block);
        let mut incoming = b.block_params(block).to_vec().into_iter();

        let (ret_ty, ret_plan) = &e.ret;
        let sret = if matches!(ret_plan, CRet::Indirect) { incoming.next() } else { None };
        // Where the body writes a struct it returns through a destination
        // pointer: straight into C's memory when C lent some.
        let through_dst = crate::builder::FunctionBuilder::returns_through_dst(ret_ty);
        let dst = through_dst.then(|| sret.unwrap_or_else(|| slot(&mut b, ret_ty.size())));

        let mut args = Vec::with_capacity(e.params.len() + 1);
        args.extend(dst);
        for (ty, arg) in &e.params {
            match arg {
                // A struct C split across registers is put back together here,
                // and the body gets its address, as every Rask caller passes one.
                CArg::Registers(pieces) => {
                    let addr = slot(&mut b, ty.size());
                    for p in pieces {
                        let v = incoming.next().expect("one block param per piece");
                        b.ins().store(MemFlags::new(), v, addr, p.offset as i32);
                    }
                    args.push(addr);
                }
                // Already an address: of C's stack copy, or of the copy C made.
                CArg::StackCopy(_) | CArg::ByAddress(_) | CArg::Scalar => {
                    args.push(incoming.next().expect("one block param per argument"));
                }
                CArg::Unsupported => unreachable!("refused by c_entry_signature"),
            }
        }
        let call = b.ins().call(body, &args);
        let results = b.inst_results(call).to_vec();

        match ret_plan {
            CRet::Scalar => {
                b.ins().return_(&results);
            }
            CRet::Registers(pieces) => {
                // A struct of a word or less comes back from the body as its
                // bytes; a wider one is already at `dst`.
                let addr = match dst {
                    Some(d) => d,
                    None => {
                        let s = slot(&mut b, 8);
                        b.ins().store(MemFlags::new(), results[0], s, 0);
                        s
                    }
                };
                let vals: Vec<Value> = pieces
                    .iter()
                    .map(|p| b.ins().load(piece_type(p.kind), MemFlags::new(), addr, p.offset as i32))
                    .collect();
                b.ins().return_(&vals);
            }
            CRet::Indirect => {
                if !through_dst {
                    // A struct of a word or less that C still wants in memory
                    // (Win64's 3-, 5-, 6- and 7-byte ones).
                    let s = slot(&mut b, 8);
                    b.ins().store(MemFlags::new(), results[0], s, 0);
                    let to = sret.expect("indirect has a destination");
                    crate::builder::copy_bytes(&mut b, s, 0, to, 0, ret_ty.size());
                }
                b.ins().return_(&[]);
            }
            CRet::Unsupported => unreachable!("refused by c_entry_signature"),
        }
        b.finalize();
        self.module
            .define_function(e.thunk, &mut self.ctx)
            .map_err(|err| CodegenError::CraneliftError(format!("C entry: {:?}", err)))?;
        self.module.clear_context(&mut self.ctx);
        Ok(())
    }
}
