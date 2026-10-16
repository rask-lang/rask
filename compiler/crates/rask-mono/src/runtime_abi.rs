// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The integers the C runtime and the compiler both read: status codes, kind
//! tags, encodings.
//!
//! This list is the only copy. `runtime/rask_abi.h` is generated from it and
//! the runtime includes that header; `runtime_abi::tests` fails when the
//! checked-in header is stale, and `RASK_BLESS=1 cargo test -p rask-mono
//! runtime_abi` rewrites it. Each pair used to be written out on both sides
//! with a "must match" comment, and nothing checked them (#1024, #1567).
//!
//! What the encodings mean, and how the runtime walks them, is documented
//! where the runtime does the walking, in `rask_runtime.h`.

macro_rules! runtime_constants {
    ($( $(#[doc = $doc:literal])* $name:ident = $value:expr; )*) => {
        $( $(#[doc = $doc])* pub const $name: i64 = $value; )*

        /// Every constant above, by name, in order.
        pub const RUNTIME_CONSTANTS: &[(&str, i64, &[&str])] = &[
            $( (stringify!($name), $name, &[$($doc),*]) ),*
        ];
    };
}

runtime_constants! {
    /// `T?` and `T or E`: the tag of the present side (`Some`, `Ok`).
    RASK_TAG_PRESENT = rask_stdlib::wrappers::PRESENT_TAG as i64;
    /// The tag of the absent or failed side (`None`, `Err`).
    RASK_TAG_ABSENT = rask_stdlib::wrappers::ABSENT_TAG as i64;

    /// A `RaskStr` holds up to this many bytes inline. Its last byte is the
    /// inline capacity left, so an empty string's is this.
    RASK_SSO_MAX = 15;
    /// The bit of a `RaskStr`'s second word that marks the heap form.
    RASK_STRING_HEAP_BIT = 63;

    /// What a box's payload is, so the box's last release can give it back.
    RASK_BOX_PAYLOAD_NONE = 0;
    RASK_BOX_PAYLOAD_VEC = 1;
    RASK_BOX_PAYLOAD_MAP = 2;
    /// A closure block (#1253).
    RASK_BOX_PAYLOAD_CLOSURE = 3;
    /// Another box: freed through its own header (#1302).
    RASK_BOX_PAYLOAD_BOX = 4;

    /// An owned-entry word: the byte offset in the low 28 bits, the kind in
    /// the top 4.
    RASK_OWNED_KIND_SHIFT = 28;
    RASK_OWNED_OFFSET_MASK = 0x0FFF_FFFF;
    RASK_OWNED_STRING = 0;
    RASK_OWNED_VEC = 1;
    RASK_OWNED_MAP = 2;
    /// A guard: the entries after it apply when a tag holds one value.
    RASK_OWNED_TAG_IF = 3;
    RASK_OWNED_CLOSURE = 4;
    RASK_OWNED_TRAITBOX = 5;
    RASK_OWNED_HEAP = 6;
    /// The value here is described by the list being walked, from its start.
    RASK_OWNED_SELF = 7;

    /// Word index of a vtable's `owned_release` (`rask_mir::vtable_layout`).
    RASK_VTABLE_OWNED_RELEASE_WORD = 2;

    /// What a rack node's link field holds: `Link<T>` / `Link<T>?`,
    /// `Vec<Link<T>>`, `Map<K, Link<T>>`.
    RASK_RACK_FIELD_LINK = 0;
    RASK_RACK_FIELD_VEC = 1;
    RASK_RACK_FIELD_MAP = 2;

    /// How a string-out-param call ended.
    RASK_STROUT_OK = 0;
    /// The error out-param holds the message: `IoError.Other(msg)`.
    RASK_STROUT_ERROR = 1;
    /// The input ran out: `IoError.UnexpectedEof`.
    RASK_STROUT_EOF = 2;

    /// A parsed JSON value's kind.
    RASK_JSON_NULL = 0;
    RASK_JSON_BOOL = 1;
    RASK_JSON_NUM = 2;
    RASK_JSON_STR = 3;
    RASK_JSON_ARR = 4;
    RASK_JSON_OBJ = 5;

    /// `rask_json_decode_into`'s status, in `JsonError` variant order after OK.
    RASK_JSON_OK = 0;
    RASK_JSON_ERR_PARSE = 1;
    RASK_JSON_ERR_TYPE = 2;
    RASK_JSON_ERR_MISSING = 3;

    /// A decode shape's kind.
    RASK_JSHAPE_BOOL = 0;
    RASK_JSHAPE_I8 = 1;
    RASK_JSHAPE_I16 = 2;
    RASK_JSHAPE_I32 = 3;
    RASK_JSHAPE_I64 = 4;
    RASK_JSHAPE_U8 = 5;
    RASK_JSHAPE_U16 = 6;
    RASK_JSHAPE_U32 = 7;
    RASK_JSHAPE_U64 = 8;
    RASK_JSHAPE_F32 = 9;
    RASK_JSHAPE_F64 = 10;
    RASK_JSHAPE_STRING = 11;
    RASK_JSHAPE_PRIM_COUNT = 12;
    RASK_JSHAPE_VEC = 12;
    RASK_JSHAPE_MAP = 13;
    RASK_JSHAPE_OPT = 14;
    RASK_JSHAPE_STRUCT = 15;

    /// A decode shape field flag: `@no_serialize`, never read from the input.
    RASK_JFIELD_NOT_SERIALIZED = 1;

    /// How a joined task ended.
    RASK_JOIN_OK = 0;
    RASK_JOIN_PANICKED = 1;

    /// A channel operation's status.
    RASK_CHAN_OK = 0;
    RASK_CHAN_CLOSED = -1;
    RASK_CHAN_FULL = -2;
    RASK_CHAN_EMPTY = -3;
    RASK_CHAN_CANCELLED = -4;
}

/// `runtime/rask_abi.h`, as this list says it should read.
pub fn abi_header() -> String {
    let mut out = String::from(
        "// SPDX-License-Identifier: (MIT OR Apache-2.0)\n\
         //\n\
         // Generated from `rask_mono::runtime_abi`. Don't edit it here: change the\n\
         // table there and run `RASK_BLESS=1 cargo test -p rask-mono runtime_abi`.\n\
         \n\
         #ifndef RASK_ABI_H\n\
         #define RASK_ABI_H\n",
    );
    for (name, value, doc) in RUNTIME_CONSTANTS {
        if !doc.is_empty() {
            out.push('\n');
        }
        for line in *doc {
            out.push_str("//");
            out.push_str(line);
            out.push('\n');
        }
        if *value < 0 {
            out.push_str(&format!("#define {name} ({value})\n"));
        } else {
            out.push_str(&format!("#define {name} {value}\n"));
        }
    }
    out.push_str("\n#endif\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/rask_abi.h")
    }

    #[test]
    fn runtime_abi_header_is_current() {
        let want = abi_header();
        let path = header_path();
        if std::env::var_os("RASK_BLESS").is_some() {
            std::fs::write(&path, &want).expect("write rask_abi.h");
            return;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "runtime/rask_abi.h is stale. Run `RASK_BLESS=1 cargo test -p rask-mono runtime_abi`"
        );
    }

    #[test]
    fn runtime_sources_define_none_of_these_themselves() {
        let runtime = header_path().parent().unwrap().to_path_buf();
        for entry in std::fs::read_dir(&runtime).unwrap() {
            let path = entry.unwrap().path();
            let ext = path.extension().and_then(|e| e.to_str());
            if !matches!(ext, Some("c") | Some("h")) || path.ends_with("rask_abi.h") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (name, _, _) in RUNTIME_CONSTANTS {
                let define = format!("#define {name} ");
                assert!(
                    !text.lines().any(|l| l.trim_start().starts_with(&define)),
                    "{} defines {name}, which rask_abi.h owns",
                    path.display()
                );
            }
        }
    }
}
