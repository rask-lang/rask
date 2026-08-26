// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Raw pointers for the interpreter (mem.unsafe).
//!
//! The interpreter has no addressable memory to point into, so a raw pointer
//! here is a *provenance*: which buffer it came from plus an element index.
//! `p.offset(1)` moves the index, `*p` reads the element, and both agree with
//! what the same program does natively, where the pointer really is an address
//! and `offset` really is address arithmetic.
//!
//! Before this the interpreter had no pointer value at all — `as_ptr()` handed
//! back the integer 0, `*p` folded to 0, and `read`/`offset` were "no method on
//! `i64`" (#935). The dereference was the bad half: 0 is a plausible byte, so
//! FFI or buffer code ran to completion on `--interp` and answered wrong.
//!
//! Two things the address model gives that an index alone can't: `is_aligned`
//! and friends need a number to take a remainder of, and printing a pointer has
//! to produce something that looks like an address. Both come from `addr()`,
//! which anchors on the target's real allocation. Pointer *identity* — `eq`,
//! `ne` — doesn't go through it, since target-and-index is the exact answer.

use std::sync::{Arc, Mutex};

use rask_stdlib::ptr_methods::{self, PtrSig};

use crate::interp::RuntimeError;
use crate::value::{IntKind, Value, VecData};

/// What a pointer points into.
#[derive(Debug, Clone)]
pub enum PtrTarget {
    /// A string's UTF-8 bytes. Native's buffer is NUL-terminated, so index
    /// `len` is a readable 0 rather than off the end.
    Bytes(Arc<Mutex<String>>),
    /// A Vec's elements, addressed by index.
    Elements(Arc<Mutex<VecData>>),
    /// An address with no interpreter object behind it — `null`, or anything
    /// that arrived from C. Reading through one is an error, not a guess.
    Foreign(u64),
}

/// A `*T`. `index` counts elements, matching the way native's pointer
/// arithmetic scales by the pointee's size rather than by bytes.
#[derive(Debug, Clone)]
pub struct RawPtr {
    pub target: PtrTarget,
    /// Signed, and allowed to sit outside the buffer: native computes
    /// `p.sub(1)` off the front without complaint and only faults on the read.
    pub index: i64,
}

impl RawPtr {
    pub fn null() -> RawPtr {
        RawPtr { target: PtrTarget::Foreign(0), index: 0 }
    }

    pub fn bytes(s: &Arc<Mutex<String>>) -> RawPtr {
        RawPtr { target: PtrTarget::Bytes(Arc::clone(s)), index: 0 }
    }

    pub fn elements(v: &Arc<Mutex<VecData>>) -> RawPtr {
        RawPtr { target: PtrTarget::Elements(Arc::clone(v)), index: 0 }
    }

    fn moved_by(&self, n: i64) -> RawPtr {
        RawPtr { target: self.target.clone(), index: self.index.wrapping_add(n) }
    }

    /// Bytes per element, which is also the stride `addr()` walks in.
    fn stride(&self) -> u64 {
        match self.target {
            PtrTarget::Bytes(_) => 1,
            // Every Vec element sits in an 8-byte slot natively.
            PtrTarget::Elements(_) => 8,
            PtrTarget::Foreign(_) => 1,
        }
    }

    /// A stand-in address, for the questions that need a number: alignment and
    /// printing. Anchored on the target's own allocation, so it is stable
    /// within a run, distinct between objects, and 8-aligned at index 0 —
    /// which is what native's `malloc`'d buffers give too.
    pub fn addr(&self) -> u64 {
        let base = match &self.target {
            PtrTarget::Bytes(s) => Arc::as_ptr(s) as *const u8 as u64,
            PtrTarget::Elements(v) => Arc::as_ptr(v) as *const u8 as u64,
            PtrTarget::Foreign(a) => *a,
        };
        base.wrapping_add((self.index.wrapping_mul(self.stride() as i64)) as u64)
    }

    /// Pointer identity: the same place in the same buffer. Two `as_ptr()`
    /// calls on one string are equal; a pointer into a different string is
    /// not, whatever the two synthetic addresses happen to be.
    pub fn same_place(&self, other: &RawPtr) -> bool {
        if self.index != other.index {
            return false;
        }
        match (&self.target, &other.target) {
            (PtrTarget::Bytes(a), PtrTarget::Bytes(b)) => Arc::ptr_eq(a, b),
            (PtrTarget::Elements(a), PtrTarget::Elements(b)) => Arc::ptr_eq(a, b),
            (PtrTarget::Foreign(a), PtrTarget::Foreign(b)) => a == b,
            _ => false,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.target, PtrTarget::Foreign(0)) && self.index == 0
    }

    /// The name of the buffer, for error messages.
    fn describe_target(&self) -> String {
        match &self.target {
            PtrTarget::Bytes(s) => {
                format!("a {}-byte string buffer", s.lock().unwrap().len())
            }
            PtrTarget::Elements(v) => {
                format!("a {}-element Vec", v.lock().unwrap().items.len())
            }
            PtrTarget::Foreign(_) => "memory outside the interpreter".to_string(),
        }
    }

    fn out_of_range(&self, verb: &str) -> RuntimeError {
        if self.is_null() {
            return RuntimeError::Panic(format!("null pointer {}", verb));
        }
        RuntimeError::Panic(format!(
            "raw pointer {} at element {}, outside {}",
            verb,
            self.index,
            self.describe_target()
        ))
    }

    /// `*p` and `p.read()` — the same operation, spelled two ways.
    ///
    /// mem.unsafe's debug-mode table says an out-of-bounds pointer panics with
    /// a location rather than reading whatever is there. The interpreter is a
    /// debug environment by construction, so it always takes that branch.
    pub fn read(&self) -> Result<Value, RuntimeError> {
        match &self.target {
            PtrTarget::Bytes(s) => {
                let guard = s.lock().unwrap();
                let bytes = guard.as_bytes();
                match self.index {
                    // The NUL native's buffer carries, so walking to the
                    // terminator answers 0 here as well.
                    i if i == bytes.len() as i64 => Ok(Value::Int(0, IntKind::U8)),
                    i if i >= 0 && i < bytes.len() as i64 => {
                        Ok(Value::Int(bytes[i as usize] as i64, IntKind::U8))
                    }
                    _ => {
                        drop(guard);
                        Err(self.out_of_range("read"))
                    }
                }
            }
            PtrTarget::Elements(v) => {
                let guard = v.lock().unwrap();
                let found = usize::try_from(self.index)
                    .ok()
                    .and_then(|i| guard.items.get(i))
                    .cloned();
                drop(guard);
                found.ok_or_else(|| self.out_of_range("read"))
            }
            PtrTarget::Foreign(_) => Err(self.out_of_range("read")),
        }
    }

    /// `p.write(v)`. Writes land in the buffer the pointer came from, so a
    /// `Vec` written through `as_mut_ptr()` is changed for every name that
    /// holds it — the same aliasing native has.
    pub fn write(&self, val: Value) -> Result<(), RuntimeError> {
        match &self.target {
            PtrTarget::Bytes(s) => {
                let byte = match &val {
                    Value::Int(n, _) if (0..=255).contains(n) => *n as u8,
                    other => {
                        return Err(RuntimeError::TypeError(format!(
                            "writing through a `*u8` needs a byte, got {}",
                            other.type_name()
                        )))
                    }
                };
                let mut guard = s.lock().unwrap();
                let mut bytes = guard.as_bytes().to_vec();
                if self.index < 0 || self.index >= bytes.len() as i64 {
                    drop(guard);
                    return Err(self.out_of_range("write"));
                }
                bytes[self.index as usize] = byte;
                // Native would take the byte and leave the string malformed.
                // Rust's `String` can't hold that, and silently dropping the
                // write would be worse than saying so.
                match String::from_utf8(bytes) {
                    Ok(s) => {
                        *guard = s;
                        Ok(())
                    }
                    Err(_) => {
                        drop(guard);
                        Err(RuntimeError::Panic(format!(
                            "writing {} at byte {} would leave the string invalid UTF-8 \
                             — the interpreter can't hold a malformed string",
                            byte, self.index
                        )))
                    }
                }
            }
            PtrTarget::Elements(v) => {
                let mut guard = v.lock().unwrap();
                let len = guard.items.len() as i64;
                if self.index < 0 || self.index >= len {
                    drop(guard);
                    return Err(self.out_of_range("write"));
                }
                guard.items[self.index as usize] = val;
                Ok(())
            }
            PtrTarget::Foreign(_) => Err(self.out_of_range("write")),
        }
    }

    /// The bytes from here to the end of the buffer — what `string.from_raw`
    /// and `string.from_c` read back out.
    pub fn bytes_from_here(&self) -> Result<Vec<u8>, RuntimeError> {
        match &self.target {
            PtrTarget::Bytes(s) => {
                let guard = s.lock().unwrap();
                let bytes = guard.as_bytes();
                if self.index < 0 || self.index > bytes.len() as i64 {
                    drop(guard);
                    return Err(self.out_of_range("read"));
                }
                Ok(bytes[self.index as usize..].to_vec())
            }
            // Native just reads bytes at the address, whatever the buffer
            // holds. The interpreter has values rather than bytes, so there is
            // nothing here to reinterpret — say which pointer it got and what
            // it can work with, instead of naming only the expected shape (#1012).
            PtrTarget::Elements(_) => Err(RuntimeError::TypeError(format!(
                "reading a string out of {} would mean reinterpreting its elements as \
                 bytes, which the interpreter can't do — it holds values, not memory \
                 (#1012). Pass a pointer from `string.as_ptr()`.",
                self.describe_target()
            ))),
            PtrTarget::Foreign(_) => Err(RuntimeError::TypeError(
                "reading a string through a pointer that came from outside the \
                 interpreter — there are no bytes behind it to read"
                    .to_string(),
            )),
        }
    }
}

/// Dispatch a method on a `*T`.
///
/// The method set comes from `rask_stdlib::ptr_methods` — the same table the
/// type checker, MIR lowering and codegen read — so a method added there
/// surfaces here as "not implemented" rather than quietly not existing. That
/// was the shape of the original bug: four hand-kept copies of this list, and
/// the interpreter had no copy at all.
pub(crate) fn call_ptr_method(
    p: &RawPtr,
    method: &str,
    args: Vec<Value>,
) -> Result<Value, RuntimeError> {
    // Rendering, the same way every other primitive answers it. A pointer
    // prints as its address (see `Display for Value`), which is what native
    // does; without this `println("{p}")` was "no method on `raw pointer`".
    if matches!(method, "to_string" | "debug_string") {
        return Ok(Value::String(Arc::new(Mutex::new(p.addr().to_string()))));
    }

    let Some(spec) = ptr_methods::lookup(method) else {
        return Err(RuntimeError::NoSuchMethod {
            ty: "raw pointer".to_string(),
            method: method.to_string(),
        });
    };

    let int_arg = |i: usize| -> Result<i64, RuntimeError> {
        match args.get(i) {
            Some(Value::Int(n, _)) => Ok(*n),
            Some(other) => Err(RuntimeError::TypeError(format!(
                "`{}` expects an integer, got {}",
                method,
                other.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch { expected: i + 1, got: args.len() }),
        }
    };
    let ptr_arg = |i: usize| -> Result<&RawPtr, RuntimeError> {
        match args.get(i) {
            Some(Value::RawPtr(q)) => Ok(q),
            Some(other) => Err(RuntimeError::TypeError(format!(
                "`{}` expects a pointer, got {}",
                method,
                other.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch { expected: i + 1, got: args.len() }),
        }
    };

    match spec.sig {
        PtrSig::Read => p.read(),
        PtrSig::Write => {
            let val = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                expected: 1,
                got: 0,
            })?;
            p.write(val)?;
            Ok(Value::Unit)
        }
        // `add` and `offset` step forward, `sub` steps back. The step is in
        // elements, which is why nothing here mentions a byte size.
        PtrSig::Arith => {
            let n = int_arg(0)?;
            let step = if method == "sub" { n.wrapping_neg() } else { n };
            Ok(Value::RawPtr(p.moved_by(step)))
        }
        PtrSig::Predicate => match method {
            "is_null" => Ok(Value::Bool(p.is_null())),
            "is_aligned" => Ok(Value::Bool(p.addr() % 8 == 0)),
            _ => Err(unimplemented_ptr_method(method)),
        },
        PtrSig::PredicateInt => {
            let n = int_arg(0)?;
            Ok(Value::Bool(n > 0 && p.addr() % n as u64 == 0))
        }
        PtrSig::Comparison => {
            let same = p.same_place(ptr_arg(0)?);
            match method {
                "eq" => Ok(Value::Bool(same)),
                "ne" => Ok(Value::Bool(!same)),
                _ => Err(unimplemented_ptr_method(method)),
            }
        }
        PtrSig::ToInt => {
            let n = int_arg(0)?;
            if n <= 0 {
                return Ok(Value::Int(0, IntKind::usize_kind()));
            }
            let rem = p.addr() % n as u64;
            let offset = if rem == 0 { 0 } else { n as u64 - rem };
            Ok(Value::Int(offset as i64, IntKind::usize_kind()))
        }
        // Type-only natively — no runtime call, so nothing to do here either.
        //
        // Known gap, filed as #1012: natively a cast changes the width of the
        // following read, so `*p.cast<u8>()` on a `*i64` holding 300 gives 44,
        // its low byte. The interpreter carries values rather than bytes and
        // has nothing to narrow, so it answers 300. Closing it needs the target
        // type the call was written with, which is what #986 drops.
        PtrSig::Cast => Ok(Value::RawPtr(p.clone())),
    }
}

fn unimplemented_ptr_method(method: &str) -> RuntimeError {
    RuntimeError::Generic(format!(
        "`{}` is in the pointer method table but the interpreter has no implementation \
         for it — add one in rask-interp/src/ptr.rs",
        method
    ))
}
