// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Raw pointer operations for unsafe code.
// All values are currently i64 (8 bytes).

#include <stdint.h>

int64_t rask_ptr_add(int64_t ptr, int64_t n) {
    return ptr + n * 8;
}

int64_t rask_ptr_sub(int64_t ptr, int64_t n) {
    return ptr - n * 8;
}

int64_t rask_ptr_offset(int64_t ptr, int64_t n) {
    return ptr + n * 8;
}

int64_t rask_ptr_read(int64_t ptr) {
    return *(int64_t *)(uintptr_t)ptr;
}

void rask_ptr_write(int64_t ptr, int64_t val) {
    *(int64_t *)(uintptr_t)ptr = val;
}

int64_t rask_ptr_is_null(int64_t ptr) {
    return ptr == 0;
}

int64_t rask_ptr_is_aligned(int64_t ptr) {
    return (ptr % 8) == 0;
}

int64_t rask_ptr_is_aligned_to(int64_t ptr, int64_t n) {
    return n > 0 && (ptr % n) == 0;
}

int64_t rask_ptr_align_offset(int64_t ptr, int64_t n) {
    if (n <= 0) return 0;
    int64_t rem = ptr % n;
    return rem == 0 ? 0 : n - rem;
}
