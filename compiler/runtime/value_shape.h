// SPDX-License-Identifier: (MIT OR Apache-2.0)

// What a shape is made of. A call site describes a type to the runtime as a
// shape (`rask_json_shape_*`), since codegen leaves no type information behind:
// json.c decodes and encodes through one, and claims.c builds a claim's inputs
// from one. Both walk the same tree.

#ifndef RASK_VALUE_SHAPE_H
#define RASK_VALUE_SHAPE_H

#include "rask_runtime.h"

struct RaskJsonField {
    char *key;
    int64_t offset;
    RaskJsonShape *shape;   // NULL for a field that isn't serialized
    int64_t flags;
    RaskJsonFill fill;      // the field's default, or NULL when it has none
};

// One enum variant: its tag, and its payload as a struct whose field offsets
// count from the start of the enum. NULL payload for a variant with none.
struct RaskJsonVariant {
    int64_t tag;
    RaskJsonShape *payload;
};

struct RaskJsonShape {
    int64_t kind;
    int64_t slot;                 // struct/enum byte size, or element/value slot width
    int64_t payload_offset;       // T?: where the payload sits, past the tag
    int      is_static;           // primitive singletons are never freed
    RaskJsonShape *elem;          // Vec element / Map value / T? payload
    // What a Vec element or Map value owns, in the form every container
    // constructor takes, so the container the decoder builds frees it.
    const int32_t *elem_owned;
    int64_t n_elem_owned;
    int64_t elem_align;
    struct RaskJsonField *fields; // struct fields
    int32_t nfields;
    int32_t cap;
    struct RaskJsonVariant *variants; // enum variants
    int32_t nvariants;
    int32_t vcap;
    int64_t tag_offset;           // enum: where the tag sits, and how wide it is
    int64_t tag_width;
};

#endif
