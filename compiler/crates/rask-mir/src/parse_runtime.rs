// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The runtime entries behind `string.parse<T>`.
//!
//! The runtime has one parse per width, because each narrow one range-checks
//! against its own type (`"300".parse<u8>()` is out of range, not 44). Lowering
//! picks the entry from the call's checked result type and says so by name;
//! this table is the one place those names are written, and codegen asks it
//! what a parse entry does rather than reading the spelling.

use crate::MirType;

/// What a parse entry stores through its out-parameter. Codegen loads it back
/// at that type before converting to the result slot's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseWrites {
    Int,
    Float,
}

/// `(entry, what it writes)`. `string_parse` is the one the declaration
/// names, kept for a result slot the checker left open (a generic body not
/// yet instantiated).
const ENTRIES: &[(&str, ParseWrites)] = &[
    ("string_parse", ParseWrites::Int),
    ("string_parse_i8", ParseWrites::Int),
    ("string_parse_i16", ParseWrites::Int),
    ("string_parse_i32", ParseWrites::Int),
    ("string_parse_i64", ParseWrites::Int),
    ("string_parse_u8", ParseWrites::Int),
    ("string_parse_u16", ParseWrites::Int),
    ("string_parse_u32", ParseWrites::Int),
    ("string_parse_u64", ParseWrites::Int),
    ("string_parse_f32", ParseWrites::Float),
    ("string_parse_f64", ParseWrites::Float),
];

/// The entry that writes a result slot whose ok side is `ok`. `None` for a
/// payload the runtime has no parse for.
pub fn entry_for_slot(ok: &MirType) -> Option<&'static str> {
    Some(match ok {
        MirType::I8 => "string_parse_i8",
        MirType::I16 => "string_parse_i16",
        MirType::I32 => "string_parse_i32",
        MirType::I64 => "string_parse_i64",
        MirType::U8 => "string_parse_u8",
        MirType::U16 => "string_parse_u16",
        MirType::U32 => "string_parse_u32",
        MirType::U64 => "string_parse_u64",
        MirType::F32 => "string_parse_f32",
        MirType::F64 => "string_parse_f64",
        _ => return None,
    })
}

/// What the runtime entry `name` writes, if it is a parse entry.
pub fn writes(name: &str) -> Option<ParseWrites> {
    ENTRIES.iter().find(|(n, _)| *n == name).map(|(_, w)| *w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_entry_is_listed() {
        for ok in [
            MirType::I8, MirType::I16, MirType::I32, MirType::I64, MirType::U8,
            MirType::U16, MirType::U32, MirType::U64, MirType::F32, MirType::F64,
        ] {
            let name = entry_for_slot(&ok).unwrap();
            let w = writes(name).unwrap_or_else(|| panic!("{name} is not in ENTRIES"));
            assert_eq!(w == ParseWrites::Float, matches!(ok, MirType::F32 | MirType::F64));
        }
    }
}
