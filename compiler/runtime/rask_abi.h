// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// Generated from `rask_mono::runtime_abi`. Don't edit it here: change the
// table there and run `RASK_BLESS=1 cargo test -p rask-mono runtime_abi`.

#ifndef RASK_ABI_H
#define RASK_ABI_H

// `T?` and `T or E`: the tag of the present side (`Some`, `Ok`).
#define RASK_TAG_PRESENT 0

// The tag of the absent or failed side (`None`, `Err`).
#define RASK_TAG_ABSENT 1

// Where a `T or E` keeps the source line its error came from: a
// `uint32_t`, zero for none (`rask_types::abi`).
#define RASK_RESULT_ORIGIN_LINE_OFFSET 4

// How the runtime reads one element of a vector it formats or sorts. The
// vector only records a width, and a four-byte element could be an i32,
// a u32 or an f32 — which print and order differently — so the caller
// says which.
#define RASK_ELEM_KIND_SIGNED 0
#define RASK_ELEM_KIND_UNSIGNED 1
#define RASK_ELEM_KIND_FLOAT 2
#define RASK_ELEM_KIND_BOOL 3
#define RASK_ELEM_KIND_STRING 4
#define RASK_ELEM_KIND_CHAR 5

// How a sort hands its comparator two elements. An integer goes in a
// register, widened from the element's own width.
#define RASK_SORT_PASS_WORD 0

// A pointer to each element: aggregates and strings.
#define RASK_SORT_PASS_ADDRESS 1

// A 128-bit integer, as one value in two registers.
#define RASK_SORT_PASS_WIDE 2

// A float goes in a float register, so the comparator is called through
// a signature that says so.
#define RASK_SORT_PASS_F64 3
#define RASK_SORT_PASS_F32 4

// A `RaskStr` holds up to this many bytes inline. Its last byte is the
// inline capacity left, so an empty string's is this.
#define RASK_SSO_MAX 15

// Where a `RaskVec` keeps its element buffer and its element width.
// Codegen reads both to index a vector whose bounds check is already
// proven, without calling into the runtime.
#define RASK_VEC_DATA_OFFSET 0
#define RASK_VEC_ELEM_SIZE_OFFSET 24

// The bit of a `RaskStr`'s second word that marks the heap form.
#define RASK_STRING_HEAP_BIT 63

// The bit that marks a heap value starting partway into its buffer (a
// view). Clear, the rest of the word is the length.
#define RASK_STRING_START_BIT 62

// A heap buffer's header: a u32 count, four spare bytes, a u64
// capacity. The data follows. A `Vec<u8>` reserves the same header so
// it freezes into `bytes` without moving.
#define RASK_STRING_HEADER_SIZE 16

// What a box's payload is, so the box's last release can give it back.
#define RASK_BOX_PAYLOAD_NONE 0
#define RASK_BOX_PAYLOAD_VEC 1
#define RASK_BOX_PAYLOAD_MAP 2

// A closure block (#1253).
#define RASK_BOX_PAYLOAD_CLOSURE 3

// Another box: freed through its own header (#1302).
#define RASK_BOX_PAYLOAD_BOX 4

// An owned-entry word: the byte offset in the low 28 bits, the kind in
// the top 4.
#define RASK_OWNED_KIND_SHIFT 28
#define RASK_OWNED_OFFSET_MASK 268435455
#define RASK_OWNED_STRING 0
#define RASK_OWNED_VEC 1
#define RASK_OWNED_MAP 2

// A guard: the entries after it apply when a tag holds one value.
#define RASK_OWNED_TAG_IF 3
#define RASK_OWNED_CLOSURE 4
#define RASK_OWNED_TRAITBOX 5
#define RASK_OWNED_HEAP 6

// The value here is described by the list being walked, from its start.
#define RASK_OWNED_SELF 7

// Word indexes into a vtable (`rask_mir::vtable_layout`): the concrete
// value's size and alignment, which size an interface box's block, and
// the release for a value the box owns.
#define RASK_VTABLE_SIZE_WORD 0
#define RASK_VTABLE_ALIGN_WORD 1
#define RASK_VTABLE_OWNED_RELEASE_WORD 2

// What a rack node's link field holds: `Link<T>` / `Link<T>?`,
// `Vec<Link<T>>`, `Map<K, Link<T>>`.
#define RASK_RACK_FIELD_LINK 0
#define RASK_RACK_FIELD_VEC 1
#define RASK_RACK_FIELD_MAP 2

// How a string-out-param call ended.
#define RASK_STROUT_OK 0

// The error out-param holds the message: `IoError.Other(msg)`.
#define RASK_STROUT_ERROR 1

// The input ran out: `IoError.UnexpectedEof`.
#define RASK_STROUT_EOF 2

// A parsed JSON value's kind.
#define RASK_JSON_NULL 0
#define RASK_JSON_BOOL 1
#define RASK_JSON_NUM 2
#define RASK_JSON_STR 3
#define RASK_JSON_ARR 4
#define RASK_JSON_OBJ 5

// `rask_json_decode_into`'s status, in `JsonError` variant order after OK.
#define RASK_JSON_OK 0
#define RASK_JSON_ERR_PARSE 1
#define RASK_JSON_ERR_TYPE 2
#define RASK_JSON_ERR_MISSING 3

// A decode shape's kind.
#define RASK_JSHAPE_BOOL 0
#define RASK_JSHAPE_I8 1
#define RASK_JSHAPE_I16 2
#define RASK_JSHAPE_I32 3
#define RASK_JSHAPE_I64 4
#define RASK_JSHAPE_U8 5
#define RASK_JSHAPE_U16 6
#define RASK_JSHAPE_U32 7
#define RASK_JSHAPE_U64 8
#define RASK_JSHAPE_F32 9
#define RASK_JSHAPE_F64 10
#define RASK_JSHAPE_STRING 11
#define RASK_JSHAPE_PRIM_COUNT 12
#define RASK_JSHAPE_VEC 12
#define RASK_JSHAPE_MAP 13
#define RASK_JSHAPE_OPT 14
#define RASK_JSHAPE_STRUCT 15

// A decode shape field flag: `@no_serialize`, never read from the input.
#define RASK_JFIELD_NOT_SERIALIZED 1

// How a joined task ended.
#define RASK_JOIN_OK 0
#define RASK_JOIN_PANICKED 1

// A channel operation's status.
#define RASK_CHAN_OK 0
#define RASK_CHAN_CLOSED (-1)
#define RASK_CHAN_FULL (-2)
#define RASK_CHAN_EMPTY (-3)
#define RASK_CHAN_CANCELLED (-4)

#endif
