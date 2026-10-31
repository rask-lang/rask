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
    /// `&x`: one variable, field or element, where it lives.
    Place(crate::env::Slot),
}

/// A `*T`. `index` counts elements, matching the way native's pointer
/// arithmetic scales by the pointee's size rather than by bytes.
#[derive(Debug, Clone)]
pub struct RawPtr {
    pub target: PtrTarget,
    /// Signed, and allowed to sit outside the buffer: native computes
    /// `p.sub(1)` off the front without complaint and only faults on the read.
    ///
    /// Counted in `view`-sized steps when there is a view.
    pub index: i64,
    /// The integer a `cast` reinterprets a Vec's elements as, when it is not
    /// the element itself. Natively a cast changes how many bytes the next
    /// read takes and how far `offset` steps, so `*p.cast<u8>()` on an `*i64`
    /// holding 300 is 44, its low byte (#1012).
    pub view: Option<IntKind>,
}

impl RawPtr {
    pub fn null() -> RawPtr {
        RawPtr { target: PtrTarget::Foreign(0), index: 0, view: None }
    }

    pub fn bytes(s: &Arc<Mutex<String>>) -> RawPtr {
        RawPtr { target: PtrTarget::Bytes(Arc::clone(s)), index: 0, view: None }
    }

    pub fn elements(v: &Arc<Mutex<VecData>>) -> RawPtr {
        RawPtr { target: PtrTarget::Elements(Arc::clone(v)), index: 0, view: None }
    }

    fn moved_by(&self, n: i64) -> RawPtr {
        RawPtr { target: self.target.clone(), index: self.index.wrapping_add(n), view: self.view }
    }

    /// Bytes per step, which is also the stride `addr()` walks in.
    fn stride(&self) -> u64 {
        match &self.target {
            PtrTarget::Bytes(_) => 1,
            PtrTarget::Elements(ref v) => match self.view {
                Some(view) => view_width(view).unwrap_or(8),
                None => elem_kind(v).and_then(view_width).unwrap_or(8),
            },
            PtrTarget::Foreign(_) | PtrTarget::Place(_) => 1,
        }
    }

    /// `p.cast<U>()` on a `*T`. `from` and `to` are `T` and `U` when they are
    /// integers, `None` otherwise.
    ///
    /// Only a Vec's elements get a view: a string buffer is bytes already, and
    /// a foreign pointer has nothing behind it to read. The position is kept in
    /// bytes across the change, so `p.offset(1).cast<u8>()` on an `*i64` is
    /// byte 8, and on an `*i16` byte 2.
    pub fn cast_to(&self, from: Option<IntKind>, to: Option<IntKind>) -> Result<RawPtr, RuntimeError> {
        let PtrTarget::Elements(v) = &self.target else { return Ok(self.clone()) };
        if from == to {
            return Ok(self.clone());
        }
        // The element's own kind: from the elements, or, with no view in the
        // way, the pointee the cast starts from.
        let elem = elem_kind(v).or(if self.view.is_none() { from } else { None });
        let width = |k: Option<IntKind>| k.and_then(view_width).unwrap_or(8) as i64;
        let byte = self.index.wrapping_mul(width(self.view.or(elem)));
        // A cast back to the element type reads the element as itself.
        let view = to.filter(|k| Some(*k) != elem && view_width(*k).is_some());
        let step = width(view.or(elem));
        if byte.rem_euclid(step) != 0 {
            return Err(RuntimeError::Panic(format!(
                "casting a pointer at byte {} of {} to a {}-byte pointee leaves it \
                 between elements",
                byte,
                self.describe_target(),
                step
            )));
        }
        Ok(RawPtr { target: self.target.clone(), index: byte / step, view })
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
            PtrTarget::Place(slot) => slot.identity(),
        };
        base.wrapping_add((self.index.wrapping_mul(self.stride() as i64)) as u64)
    }

    /// Pointer identity: the same place in the same buffer. Two `as_ptr()`
    /// calls on one string are equal; a pointer into a different string is
    /// not, whatever the two synthetic addresses happen to be.
    pub fn same_place(&self, other: &RawPtr) -> bool {
        let here = self.index.wrapping_mul(self.stride() as i64);
        let there = other.index.wrapping_mul(other.stride() as i64);
        if here != there {
            return false;
        }
        match (&self.target, &other.target) {
            (PtrTarget::Bytes(a), PtrTarget::Bytes(b)) => Arc::ptr_eq(a, b),
            (PtrTarget::Elements(a), PtrTarget::Elements(b)) => Arc::ptr_eq(a, b),
            (PtrTarget::Foreign(a), PtrTarget::Foreign(b)) => a == b,
            (PtrTarget::Place(a), PtrTarget::Place(b)) => a.identity() == b.identity(),
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
            PtrTarget::Place(_) => "one value (`&x`)".to_string(),
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
                let Some(view) = self.view else {
                    let guard = v.lock().unwrap();
                    let found = usize::try_from(self.index)
                        .ok()
                        .and_then(|i| guard.items.get(i))
                        .cloned();
                    drop(guard);
                    return found.ok_or_else(|| self.out_of_range("read"));
                };
                let raw = self.view_bytes(v, "read")?;
                Ok(Value::Int(from_width(&raw, raw.len(), view.signed()), view))
            }
            PtrTarget::Foreign(_) => Err(self.out_of_range("read")),
            PtrTarget::Place(slot) if self.index == 0 => slot.get().ok_or_else(|| self.out_of_range("read")),
            PtrTarget::Place(_) => Err(self.out_of_range("read")),
        }
    }

    /// The bytes a viewed pointer covers, gathered from the elements they
    /// fall in. An element is its value's two's-complement bytes at its own
    /// width, which is how native lays a Vec out, so a `*u8` over a `Vec<i16>`
    /// walks two bytes per element and a `*u32` over a `Vec<u8>` reads four.
    fn view_bytes(&self, v: &Arc<Mutex<VecData>>, verb: &str) -> Result<Vec<u8>, RuntimeError> {
        let (start, width, ew) = self.view_span(v)?;
        let guard = v.lock().unwrap();
        let mut out = Vec::with_capacity(width);
        for b in start..start + width as i64 {
            let element = usize::try_from(b.div_euclid(ew)).ok().and_then(|i| guard.items.get(i));
            let Some(element) = element.filter(|_| b >= 0) else {
                drop(guard);
                return Err(self.out_of_range(verb));
            };
            let Value::Int(n, _) = element else {
                return Err(not_bytes(&self.describe_target(), element));
            };
            out.push((*n as u64).to_le_bytes()[b.rem_euclid(ew) as usize]);
        }
        Ok(out)
    }

    /// Write `bytes` over the span a viewed pointer covers, each element
    /// taking back the bytes that land in it at its own width and sign.
    fn write_view_bytes(&self, v: &Arc<Mutex<VecData>>, bytes: &[u8]) -> Result<(), RuntimeError> {
        let (start, _, ew) = self.view_span(v)?;
        let mut guard = v.lock().unwrap();
        for (k, byte) in bytes.iter().enumerate() {
            let b = start + k as i64;
            let element = usize::try_from(b.div_euclid(ew)).ok().filter(|_| b >= 0);
            let Some(Value::Int(n, kind)) = element.and_then(|i| guard.items.get_mut(i)) else {
                drop(guard);
                return Err(self.out_of_range("write"));
            };
            let mut raw = (*n as u64).to_le_bytes();
            raw[b.rem_euclid(ew) as usize] = *byte;
            *n = from_width(&raw, ew as usize, kind.signed());
        }
        Ok(())
    }

    /// Where a viewed pointer starts in bytes, how many it covers, and the
    /// element width it walks over.
    fn view_span(&self, v: &Arc<Mutex<VecData>>) -> Result<(i64, usize, i64), RuntimeError> {
        let width = self.stride();
        let ew = match elem_kind(v) {
            Some(kind) => view_width(kind).unwrap_or(8) as i64,
            None => return Err(self.out_of_range("read")),
        };
        Ok((self.index.wrapping_mul(width as i64), width as usize, ew))
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
            PtrTarget::Elements(v) if self.view.is_some() => {
                let Value::Int(n, _) = val else {
                    return Err(RuntimeError::TypeError(format!(
                        "writing a {} through an integer pointer",
                        val.type_name()
                    )));
                };
                let width = self.stride() as usize;
                self.write_view_bytes(v, &(n as u64).to_le_bytes()[..width])
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
            PtrTarget::Place(slot) if self.index == 0 => {
                if slot.set(val) { Ok(()) } else { Err(self.out_of_range("write")) }
            }
            PtrTarget::Place(_) => Err(self.out_of_range("write")),
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
            PtrTarget::Place(_) => Err(RuntimeError::TypeError(
                "reading a string through `&x` — it points at one value, not at bytes".to_string(),
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
    if matches!(method, "to_string" | "debug") {
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
        // The target type isn't an argument: the caller reads it off the
        // checker's type for the call and goes through `RawPtr::cast_to`. One
        // arriving here has no target to go by, so it stays as it was.
        PtrSig::Cast => Ok(Value::RawPtr(p.clone())),
    }
}

/// Bytes a view reads, for the integer widths a view can be.
fn view_width(kind: IntKind) -> Option<u64> {
    kind.bits().map(|b| (b / 8) as u64)
}

/// The integer kind a Vec's elements are, read off the elements. `None` for
/// an empty Vec or one of non-integers, which a view can't address.
fn elem_kind(v: &Arc<Mutex<VecData>>) -> Option<IntKind> {
    match v.lock().unwrap().items.first() {
        Some(Value::Int(_, kind)) => Some(*kind),
        _ => None,
    }
}

/// The value of `width` little-endian bytes, sign- or zero-extended.
fn from_width(raw: &[u8], width: usize, signed: bool) -> i64 {
    let mut word = [0u8; 8];
    word[..width].copy_from_slice(&raw[..width]);
    let unsigned = u64::from_le_bytes(word);
    let bits = (width * 8) as u32;
    if signed && bits < 64 {
        ((unsigned << (64 - bits)) as i64) >> (64 - bits)
    } else {
        unsigned as i64
    }
}

fn not_bytes(target: &str, element: &Value) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "reading part of {} through a cast pointer — the interpreter holds \
         values, not bytes, and a {} has no byte layout it can split (#1012)",
        target,
        element.type_name()
    ))
}

fn unimplemented_ptr_method(method: &str) -> RuntimeError {
    RuntimeError::Generic(format!(
        "`{}` is in the pointer method table but the interpreter has no implementation \
         for it — add one in rask-interp/src/ptr.rs",
        method
    ))
}
