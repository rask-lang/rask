// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Runtime calls lowering makes for element access, which no stdlib
//! declaration spells: `v[i]`, `m[k]`, `m[k] = v`, moving an element or a
//! field out.
//!
//! Each is the C symbol, the stdlib operation whose ownership shape it
//! shares, and how its arguments and result cross (`CallAbi`). Passes that
//! look for one of these calls compare against the symbol constants here.

use crate::call_abi::{ArgAdapt, CallAbi, RetAdapt};
use crate::FunctionRef;

/// The element's address, after a bounds check that panics.
pub const VEC_GET: &str = "rask_vec_get";
/// The element's address, for an index already proved in range.
pub const VEC_GET_UNCHECKED: &str = "rask_vec_get_unchecked";
/// The value's address by key, panicking when the key is missing.
pub const MAP_GET_UNWRAP: &str = "rask_map_get_unwrap";
/// The value's address by key, for `mutate m[k]`.
pub const MAP_BORROW_ELEM: &str = "rask_map_borrow_elem";
/// A field's bytes copied out and the slot zeroed.
pub const FIELD_TAKE: &str = "rask_field_take";
/// An element's bytes copied out and its slot zeroed.
pub const VEC_MOVE_OUT: &str = "rask_vec_move_out";

/// The key goes by address; the result is the value's address.
pub const KEY_BY_ADDRESS: CallAbi = CallAbi::new(ArgAdapt::WrapArg1, RetAdapt::None);

/// `v[i]` read: the element, loaded from the address `rask_vec_get` returns.
pub fn vec_index() -> FunctionRef {
    FunctionRef::like(VEC_GET, "Vec_get").with_abi(CallAbi::DEREF)
}

/// The element's address, for writing through.
pub fn vec_slot() -> FunctionRef {
    FunctionRef::like(VEC_GET, "Vec_get")
}

/// `m[k]` read: the value, loaded from the address the runtime returns.
pub fn map_index() -> FunctionRef {
    FunctionRef::like(MAP_GET_UNWRAP, "Map_get")
        .with_abi(CallAbi::new(ArgAdapt::WrapArg1, RetAdapt::DerefOrString))
}

/// The value's address by key, for writing through.
pub fn map_slot() -> FunctionRef {
    FunctionRef::like(MAP_GET_UNWRAP, "Map_get").with_abi(KEY_BY_ADDRESS)
}

/// `mutate m[k]`: the value's address, held until the matching release.
pub fn map_borrow_elem() -> FunctionRef {
    FunctionRef::like(MAP_BORROW_ELEM, "Map_get").with_abi(KEY_BY_ADDRESS)
}

/// `m[k] = v`: insert or replace, releasing the value it replaces. Key and
/// value go by address.
pub fn map_set() -> FunctionRef {
    FunctionRef::like("rask_map_insert", "Map_insert")
        .with_abi(CallAbi::new(ArgAdapt::WrapArg1And2, RetAdapt::None))
}

/// `let old = self.f` before a refill: the field's bytes, handed over.
pub fn field_take() -> FunctionRef {
    FunctionRef::like(FIELD_TAKE, "Vec_remove").with_abi(CallAbi::APPEND_OUT)
}

/// A consuming `for` over a vector: each element, moved out.
pub fn vec_move_out() -> FunctionRef {
    FunctionRef::like(VEC_MOVE_OUT, "Vec_remove").with_abi(CallAbi::APPEND_OUT)
}

/// A sync box's constructor: the payload goes in by address.
pub const BOX_NEW: CallAbi = CallAbi::new(ArgAdapt::WrapArg0, RetAdapt::None);
/// `set`: the new payload goes in by address.
pub const BOX_SET: CallAbi = CallAbi::new(ArgAdapt::WrapArg1, RetAdapt::None);
/// `replace`: the new payload goes in by address, the old one comes back out
/// into the caller's destination.
pub const BOX_REPLACE: CallAbi = CallAbi::new(ArgAdapt::WrapArg1AppendOutParam, RetAdapt::FromArgAdapt);
