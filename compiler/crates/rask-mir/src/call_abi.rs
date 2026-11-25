// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! How a runtime call's C arguments and result line up with the MIR call.
//!
//! MIR writes `dst = f(a, b)`, and the C function may want `b` by address,
//! a string out-parameter in front, or hand back a pointer to load from.
//! Lowering states which on the call (`FunctionRef::abi`) where it emits it,
//! because it is the pass that knows; codegen applies it without looking the
//! name up (#1555).

/// The pair a call carries: how its arguments go in and its result comes out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallAbi {
    pub args: ArgAdapt,
    pub ret: RetAdapt,
}

impl CallAbi {
    /// Arguments as they are, the result as the C function returns it.
    pub const DIRECT: CallAbi = CallAbi { args: ArgAdapt::None, ret: RetAdapt::None };
    /// A `RaskStr *out` in front, which the call fills as its result.
    pub const STRING_OUT: CallAbi = CallAbi { args: ArgAdapt::StringOutParam, ret: RetAdapt::FromArgAdapt };
    /// The C function returns a pointer to the value; the destination loads it.
    pub const DEREF: CallAbi = CallAbi { args: ArgAdapt::None, ret: RetAdapt::DerefOrString };
    /// A `void *out` at the end, which the call fills as its result.
    pub const APPEND_OUT: CallAbi = CallAbi { args: ArgAdapt::AppendOutParam, ret: RetAdapt::FromArgAdapt };
    /// The C function returns a pointer to a sync box's payload.
    pub const BOX_PAYLOAD: CallAbi = CallAbi { args: ArgAdapt::None, ret: RetAdapt::BoxPayloadPtr };
    /// A negative return is `none`, anything else `some` of it.
    pub const NEG_NONE: CallAbi = CallAbi { args: ArgAdapt::None, ret: RetAdapt::NegNone };

    pub const fn new(args: ArgAdapt, ret: RetAdapt) -> CallAbi {
        CallAbi { args, ret }
    }
}

/// How to adapt arguments before a stdlib call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArgAdapt {
    /// Pass args as-is
    #[default]
    None,
    /// Wrap args[1] as pointer (skip if string)
    WrapArg1,
    /// Wrap args[2] as pointer (skip if string)
    WrapArg2,
    /// Wrap args[1] and args[2] as pointers
    WrapArg1And2,
    /// Inject 16-byte string out-param as first arg
    StringOutParam,
    /// Two i64s written into the destination's own 16-byte slot — a
    /// `(usize, usize)` tuple, start at +0 and end at +8.
    PairOutParam,
    /// Same, but the call also returns a 0/1 status that becomes the
    /// `string or E` tag. The string goes to a scratch slot rather than to
    /// dst, because dst is the Result and the string is only its payload.
    StringResultOutParam,
    /// Append 8-byte (or 16-byte for string dst) out-param
    AppendOutParam,
    /// WrapArg2, then AppendOutParam: a value goes in by address and the one
    /// it displaced comes back out (`Vec.replace`).
    WrapArg2AppendOutParam,
    /// Append the destination `T?`'s payload address as an out-param. The call
    /// returns 1 (wrote a value) or 0 (nothing there), which becomes the tag.
    /// For anything that hands an element back out of a container it's about to
    /// free — the runtime copies while the element is still live, instead of
    /// returning a pointer into freed storage.
    OptionOutParam,
}

/// How to adapt the return value after a stdlib call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RetAdapt {
    /// Use return value as-is (or no return)
    #[default]
    None,
    /// Load i64 from void* (or copy 16B string if dst is string type)
    DerefOrString,
    /// NULL→None(tag=1), non-NULL→Some(tag=0, deref)
    DerefOption,
    /// Determined by ArgAdapt (StringOutParam → slot addr, AppendOutParam → slot load)
    FromArgAdapt,
    /// C FFI convention: a negative scalar return means Err, otherwise Ok.
    /// Codegen wraps the return into the destination `T or E` Result slot.
    NegErr,
    /// Same convention for an Option: a negative scalar return means `none`,
    /// otherwise `some(value)`. Distinct from NegErr because Option and Result
    /// put their payloads at different offsets.
    NegNone,
    /// The return is a pointer to a sync box's payload (or, for staged access,
    /// to the working copy standing in for it). A payload that lives in its own
    /// storage binds the destination straight to that pointer, so the block's
    /// field writes land in the box rather than in a copied stack slot; anything
    /// word-sized takes one load. Which of the two is the destination type's
    /// business, so the entry only has to say "this is a payload pointer".
    ///
    /// Was a hardcoded list of four acquire names in `builder.rs`. Adding the
    /// staged pair to it was the obvious move and the wrong one — the fact
    /// belongs to the entry, and a name missing from a list four deep is how
    /// this went wrong the first time.
    BoxPayloadPtr,
    /// The return is a status: 0 for Ok, or a `RASK_CHAN_*` failure the
    /// destination's error enum names a variant for (`Closed`, `Cancelled`).
    Status,
}
