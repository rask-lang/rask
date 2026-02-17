// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask C runtime — data structures and utilities for native-compiled programs.
// Linked with object files produced by rask-codegen.

#ifndef RASK_RUNTIME_H
#define RASK_RUNTIME_H

#include <stdint.h>
#include <stddef.h>

// ─── Allocator ──────────────────────────────────────────────
// Swappable allocator with optional stats tracking.
// Default uses malloc/realloc/free. Call rask_allocator_set() before any
// allocations to swap in a custom allocator.

typedef struct {
    void *(*alloc)(int64_t size, void *ctx);
    void *(*realloc)(void *ptr, int64_t old_size, int64_t new_size, void *ctx);
    void  (*free)(void *ptr, void *ctx);
    void  *ctx;
} RaskAllocator;

typedef struct {
    int64_t alloc_count;
    int64_t free_count;
    int64_t bytes_allocated;
    int64_t bytes_freed;
    int64_t peak_bytes;
} RaskAllocStats;

void  rask_allocator_set(const RaskAllocator *a);
void  rask_alloc_stats(RaskAllocStats *out);

// These use the active allocator (default: malloc/free).
void *rask_alloc(int64_t size);
void *rask_realloc(void *ptr, int64_t old_size, int64_t new_size);
void  rask_free(void *ptr);

// ─── Vec ────────────────────────────────────────────────────
// Growable array storing elements as raw bytes.

typedef struct RaskVec RaskVec;

RaskVec *rask_vec_new(int64_t elem_size);
RaskVec *rask_vec_with_capacity(int64_t elem_size, int64_t cap);
void     rask_vec_free(RaskVec *v);
int64_t  rask_vec_len(const RaskVec *v);
int64_t  rask_vec_capacity(const RaskVec *v);
int64_t  rask_vec_push(RaskVec *v, const void *elem);
void    *rask_vec_get(const RaskVec *v, int64_t index);
void     rask_vec_set(RaskVec *v, int64_t index, const void *elem);
int64_t  rask_vec_pop(RaskVec *v, void *out);
int64_t  rask_vec_remove(RaskVec *v, int64_t index);
void     rask_vec_clear(RaskVec *v);
int64_t  rask_vec_reserve(RaskVec *v, int64_t additional);
int64_t  rask_vec_is_empty(const RaskVec *v);
RaskVec *rask_iter_skip(const RaskVec *src, int64_t n);

// ─── String ─────────────────────────────────────────────────
// UTF-8 owned string, always null-terminated.

typedef struct RaskString RaskString;

RaskString *rask_string_new(void);
RaskString *rask_string_from(const char *s);
RaskString *rask_string_from_bytes(const char *data, int64_t len);
void        rask_string_free(RaskString *s);
int64_t     rask_string_len(const RaskString *s);
const char *rask_string_ptr(const RaskString *s);
int64_t     rask_string_push_byte(RaskString *s, uint8_t byte);
int64_t     rask_string_push_char(RaskString *s, int32_t codepoint);
int64_t     rask_string_append(RaskString *s, const RaskString *other);
int64_t     rask_string_append_cstr(RaskString *s, const char *cstr);
RaskString *rask_string_clone(const RaskString *s);
int64_t     rask_string_eq(const RaskString *a, const RaskString *b);
RaskString *rask_string_substr(const RaskString *s, int64_t start, int64_t end);
RaskString *rask_string_concat(const RaskString *a, const RaskString *b);
int64_t     rask_string_contains(const RaskString *haystack, const RaskString *needle);
RaskString *rask_string_to_lowercase(const RaskString *s);
int64_t     rask_string_starts_with(const RaskString *s, const RaskString *prefix);
int64_t     rask_string_ends_with(const RaskString *s, const RaskString *suffix);
RaskVec    *rask_string_lines(const RaskString *s);
RaskString *rask_string_trim(const RaskString *s);
RaskVec    *rask_string_split(const RaskString *s, const RaskString *sep);
RaskString *rask_string_replace(const RaskString *s, const RaskString *from, const RaskString *to);
int64_t     rask_string_parse_int(const RaskString *s);
double      rask_string_parse_float(const RaskString *s);
RaskString *rask_i64_to_string(int64_t val);
RaskString *rask_bool_to_string(int64_t val);
RaskString *rask_f64_to_string(double val);
RaskString *rask_char_to_string(int32_t codepoint);

// ─── Map ────────────────────────────────────────────────────
// Open-addressing hash map with linear probing.
// Keys and values stored as raw bytes. Uses FNV-1a hashing + memcmp by default.
// For string-keyed maps, supply custom hash/eq via rask_map_new_custom.

typedef struct RaskMap RaskMap;

typedef uint64_t (*RaskHashFn)(const void *key, int64_t key_size);
typedef int      (*RaskEqFn)(const void *a, const void *b, int64_t key_size);

RaskMap *rask_map_new(int64_t key_size, int64_t val_size);
RaskMap *rask_map_new_custom(int64_t key_size, int64_t val_size,
                             RaskHashFn hash, RaskEqFn eq);
void     rask_map_free(RaskMap *m);
int64_t  rask_map_len(const RaskMap *m);
int64_t  rask_map_insert(RaskMap *m, const void *key, const void *val);
void    *rask_map_get(const RaskMap *m, const void *key);
int64_t  rask_map_remove(RaskMap *m, const void *key);
int64_t  rask_map_contains(const RaskMap *m, const void *key);
int64_t  rask_map_is_empty(const RaskMap *m);
void     rask_map_clear(RaskMap *m);
RaskVec *rask_map_keys(const RaskMap *m);
RaskVec *rask_map_values(const RaskMap *m);

// Built-in hash/eq functions
uint64_t rask_hash_bytes(const void *key, int64_t key_size);
int      rask_eq_bytes(const void *a, const void *b, int64_t key_size);

// ─── Pool ───────────────────────────────────────────────────
// Handle-based sparse storage with generation counters.

typedef struct {
    uint32_t pool_id;
    uint32_t index;
    uint32_t generation;
} RaskHandle;

typedef struct RaskPool RaskPool;

RaskPool   *rask_pool_new(int64_t elem_size);
RaskPool   *rask_pool_with_capacity(int64_t elem_size, int64_t cap);
void        rask_pool_free(RaskPool *p);
int64_t     rask_pool_len(const RaskPool *p);
RaskHandle  rask_pool_insert(RaskPool *p, const void *elem);
void       *rask_pool_get(const RaskPool *p, RaskHandle h);
int64_t     rask_pool_remove(RaskPool *p, RaskHandle h, void *out);
int64_t     rask_pool_is_valid(const RaskPool *p, RaskHandle h);
RaskHandle  rask_pool_alloc(RaskPool *p);

// Packed i64 handle interface for codegen (index:32 | gen:32, pool_id from pool ptr)
int64_t     rask_pool_alloc_packed(RaskPool *p);
void       *rask_pool_get_packed(const RaskPool *p, int64_t packed);
int64_t     rask_pool_remove_packed(RaskPool *p, int64_t packed);
int64_t     rask_pool_is_valid_packed(const RaskPool *p, int64_t packed);

#define RASK_HANDLE_INVALID ((RaskHandle){0, UINT32_MAX, 0})

// ─── FS module ──────────────────────────────────────────────
// Higher-level file operations. Return FILE* or RaskString* as i64.

int64_t     rask_fs_open(const RaskString *path);
int64_t     rask_fs_create(const RaskString *path);
RaskString *rask_fs_canonicalize(const RaskString *path);
int64_t     rask_fs_copy(const RaskString *from, const RaskString *to);
void        rask_fs_rename(const RaskString *from, const RaskString *to);
void        rask_fs_remove(const RaskString *path);
void        rask_fs_create_dir(const RaskString *path);
void        rask_fs_create_dir_all(const RaskString *path);
void        rask_fs_append_file(const RaskString *path, const RaskString *content);

// ─── Net module ─────────────────────────────────────────────
// Basic TCP socket operations.

int64_t rask_net_tcp_listen(const RaskString *addr);

// ─── JSON module ────────────────────────────────────────────
// Encode helpers — used by codegen-generated struct serialization.

typedef struct RaskJsonBuf RaskJsonBuf;

RaskJsonBuf *rask_json_buf_new(void);
void         rask_json_buf_add_string(RaskJsonBuf *buf, const char *key, const RaskString *val);
void         rask_json_buf_add_i64(RaskJsonBuf *buf, const char *key, int64_t val);
void         rask_json_buf_add_f64(RaskJsonBuf *buf, const char *key, double val);
void         rask_json_buf_add_bool(RaskJsonBuf *buf, const char *key, int64_t val);
RaskString  *rask_json_buf_finish(RaskJsonBuf *buf);

RaskString  *rask_json_encode_string(const RaskString *s);
RaskString  *rask_json_encode_i64(int64_t val);

// Decode helpers — minimal JSON object parser.
typedef struct RaskJsonObj RaskJsonObj;

RaskJsonObj *rask_json_parse(const RaskString *s);
RaskString  *rask_json_get_string(RaskJsonObj *obj, const char *key);
int64_t      rask_json_get_i64(RaskJsonObj *obj, const char *key);
double       rask_json_get_f64(RaskJsonObj *obj, const char *key);
int8_t       rask_json_get_bool(RaskJsonObj *obj, const char *key);
int64_t      rask_json_decode(const RaskString *s);

// ─── CLI args ───────────────────────────────────────────────

void        rask_args_init(int argc, char **argv);
int64_t     rask_args_count(void);
const char *rask_args_get(int64_t index);

// ─── Panic ─────────────────────────────────────────────────
// Structured panic: aborts in main thread, catchable in spawned tasks.
// Spawned tasks use setjmp/longjmp to convert panics into JoinError.

#define RASK_PANIC_MSG_MAX 512

_Noreturn void rask_panic(const char *msg);
_Noreturn void rask_panic_fmt(const char *fmt, ...);

// Install/remove panic handler for the current thread.
// Used internally by rask_spawn — not part of the public API.
typedef struct RaskPanicCtx RaskPanicCtx;
RaskPanicCtx *rask_panic_install(void);
void          rask_panic_remove(void);

// ─── Green scheduler (M:N) ──────────────────────────────────
// Work-stealing scheduler with io_uring/epoll I/O engine.
// Tasks are stackless state machines: poll_fn(state, ctx) → 0=READY, 1=PENDING.

void      rask_runtime_init(int64_t worker_count);
void      rask_runtime_shutdown(void);

// Spawn a green task. poll_fn signature: int (*)(void *state, void *task_ctx).
// state is heap-allocated, freed by scheduler on completion.
void     *rask_green_spawn(void *poll_fn, void *state, int64_t state_size);
int64_t   rask_green_join(void *handle);
void      rask_green_detach(void *handle);
int64_t   rask_green_cancel(void *handle);

// Closure-based spawn (bridge for codegen before state machine transform).
void     *rask_green_closure_spawn(void *closure_ptr);

// Yield helpers — called by state machines to pause on I/O.
void      rask_yield_read(int fd, void *buf, size_t len);
void      rask_yield_write(int fd, const void *buf, size_t len);
void      rask_yield_accept(int listen_fd);
void      rask_yield_timeout(uint64_t ns);

// Cooperative yield — re-enqueue current task for later polling.
void      rask_yield(void);

// Check cancel flag for the current green task.
int       rask_green_task_is_cancelled(void);

// ─── Threads ───────────────────────────────────────────────
// Phase A concurrency: one OS thread per spawn (conc.strategy/A1).
// TaskHandle is affine — must be joined, detached, or cancelled.

typedef struct RaskTaskHandle RaskTaskHandle;

// Function signature for spawned tasks: takes environment pointer.
typedef void (*RaskTaskFn)(void *env);

// Spawn a new OS thread running func(env). Caller must join/detach/cancel.
RaskTaskHandle *rask_task_spawn(RaskTaskFn func, void *env);

// Block until task finishes. Returns 0 on success, -1 on panic.
// On panic, if msg_out is non-NULL, receives a heap-allocated panic message
// (caller must free). Consumes the handle.
int64_t rask_task_join(RaskTaskHandle *h, char **msg_out);

// Detach the task (fire-and-forget). Consumes the handle.
void rask_task_detach(RaskTaskHandle *h);

// Request cooperative cancellation, then wait for the task to finish.
// Returns 0 on success, -1 on panic. Consumes the handle.
int64_t rask_task_cancel(RaskTaskHandle *h, char **msg_out);

// Check if the current task has been cancelled. Returns 1 if cancelled.
int8_t rask_task_cancelled(void);

// Sleep the current thread for the given number of nanoseconds.
void rask_sleep_ns(int64_t ns);

// Codegen wrapper: spawn a task from a closure pointer [func_ptr | captures...].
// Extracts func/env, runs the task, and frees the closure allocation on completion.
RaskTaskHandle *rask_closure_spawn(void *closure_ptr);

// Simplified join: no panic message output. Returns 0 on success, -1 on panic.
int64_t rask_task_join_simple(void *h);

// ─── Channels ──────────────────────────────────────────────
// Bounded ring buffer (capacity > 0) or rendezvous (capacity == 0).
// Reference-counted sender/receiver halves. Close-on-drop.

typedef struct RaskChannel RaskChannel;
typedef struct RaskSender  RaskSender;
typedef struct RaskRecver  RaskRecver;

// Status codes for channel operations.
#define RASK_CHAN_OK     0
#define RASK_CHAN_CLOSED -1
#define RASK_CHAN_FULL   -2
#define RASK_CHAN_EMPTY  -3

// Create a channel. capacity=0 for rendezvous (unbuffered).
// Returns sender and receiver through out-params.
void rask_channel_new(int64_t elem_size, int64_t capacity,
                      RaskSender **tx_out, RaskRecver **rx_out);

// Blocking send. Copies elem_size bytes from data into the channel.
// Returns RASK_CHAN_OK or RASK_CHAN_CLOSED.
int64_t rask_channel_send(RaskSender *tx, const void *data);

// Blocking receive. Copies elem_size bytes from channel into data_out.
// Returns RASK_CHAN_OK or RASK_CHAN_CLOSED.
int64_t rask_channel_recv(RaskRecver *rx, void *data_out);

// Non-blocking variants.
int64_t rask_channel_try_send(RaskSender *tx, const void *data);
int64_t rask_channel_try_recv(RaskRecver *rx, void *data_out);

// Clone a sender (increment refcount). Multiple producers supported.
RaskSender *rask_sender_clone(RaskSender *tx);

// Drop sender/receiver. Closes the channel half when refcount hits zero.
void rask_sender_drop(RaskSender *tx);
void rask_recver_drop(RaskRecver *rx);

// i64-based channel wrappers for codegen dispatch table.
int64_t rask_channel_new_i64(int64_t capacity);
int64_t rask_channel_get_tx(int64_t pair);
int64_t rask_channel_get_rx(int64_t pair);
int64_t rask_channel_send_i64(int64_t tx, int64_t value);
int64_t rask_channel_recv_i64(int64_t rx);
void    rask_sender_drop_i64(int64_t tx);
void    rask_recver_drop_i64(int64_t rx);
int64_t rask_sender_clone_i64(int64_t tx);

// ─── Async I/O (dual-path: green task or blocking) ──────────
// Inside a green task, these submit async ops and return PENDING.
// Outside a green task, they fall back to blocking syscalls.

int64_t rask_async_read(int fd, void *buf, int64_t len);
int64_t rask_async_write(int fd, const void *buf, int64_t len);
int64_t rask_async_accept(int listen_fd);

// ─── Async channels (yield-based) ──────────────────────────
// Non-blocking try + yield loop for green tasks.
// Outside green tasks, falls back to blocking channel ops.

int64_t rask_channel_send_async(int64_t tx, int64_t value);
int64_t rask_channel_recv_async(int64_t rx);

// ─── Green-aware sleep ──────────────────────────────────────
// Yields to scheduler in green tasks, blocking nanosleep otherwise.

void rask_green_sleep_ns(int64_t ns);

// ─── Ensure hooks (LIFO cleanup) ───────────────────────────
// Per-task cleanup stack. Hooks run LIFO on cancel or panic.

typedef void (*RaskEnsureFn)(void *ctx);

void rask_ensure_push(RaskEnsureFn fn, void *ctx);
void rask_ensure_pop(void);

// ─── Mutex ─────────────────────────────────────────────────
// Exclusive access wrapper. Closure-based: data accessed only inside lock.
// Wraps pthread_mutex (conc.sync/MX1-MX2).

typedef struct RaskMutex RaskMutex;

// Callback for lock/read/write: receives pointer to the protected data.
typedef void (*RaskAccessFn)(void *data, void *ctx);

RaskMutex *rask_mutex_new(const void *initial_data, int64_t data_size);
void       rask_mutex_free(RaskMutex *m);

// Acquire lock, call f(data, ctx), release lock.
void rask_mutex_lock(RaskMutex *m, RaskAccessFn f, void *ctx);

// Non-blocking. Returns 1 if lock acquired (and f was called), 0 otherwise.
int64_t rask_mutex_try_lock(RaskMutex *m, RaskAccessFn f, void *ctx);

// ─── Shared (RwLock) ───────────────────────────────────────
// Multiple-reader / exclusive-writer wrapper (conc.sync/SY1, R1-R3).
// Wraps pthread_rwlock.

typedef struct RaskShared RaskShared;

RaskShared *rask_shared_new(const void *initial_data, int64_t data_size);
void        rask_shared_free(RaskShared *s);

// Shared read access — multiple concurrent readers allowed.
void rask_shared_read(RaskShared *s, RaskAccessFn f, void *ctx);

// Exclusive write access — blocks until all readers finish.
void rask_shared_write(RaskShared *s, RaskAccessFn f, void *ctx);

// Non-blocking variants. Return 1 if access granted, 0 otherwise.
int64_t rask_shared_try_read(RaskShared *s, RaskAccessFn f, void *ctx);
int64_t rask_shared_try_write(RaskShared *s, RaskAccessFn f, void *ctx);

#endif // RASK_RUNTIME_H
