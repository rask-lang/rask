// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `T?` and `T or E`: the variant tags, the one definition.
//!
//! Neither is a declared enum (`stdlib/option.rk` and `stdlib/result.rk`
//! declare them empty), so nothing reads the order off a declaration. MIR
//! lowering and the interpreter both take it from here. The interpreter used to
//! spell the tag at each of ~150 sites that built one of these values by hand,
//! and about a dozen gave `none` or an error the present side's tag (#1562).

/// The tag of `Some` and of `Ok`: the present side.
pub const PRESENT_TAG: u32 = 0;

/// The tag of `None` and of `Err`: the absent or failed side.
pub const ABSENT_TAG: u32 = 1;

/// The tag a `T?` or `T or E` variant carries, or `None` for any other name.
pub fn wrapper_tag(variant: &str) -> Option<u32> {
    match variant {
        "Some" | "Ok" => Some(PRESENT_TAG),
        "None" | "Err" => Some(ABSENT_TAG),
        _ => None,
    }
}
