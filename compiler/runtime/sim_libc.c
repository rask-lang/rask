// SPDX-License-Identifier: (MIT OR Apache-2.0)

// What linked C gets when it asks the world a question under sim (sim/B7, B8).
//
// The runtime's own clock, random and environment already answer from the
// seed. C linked into the program doesn't go through the runtime: a TLS
// library calls `getrandom` and `clock_gettime` itself, and a C hash table can
// key on the addresses `malloc` returns. This file defines those libc
// functions in the executable, so they win over libc's for every object and
// shared library the program links. While a sim run is active they answer
// from the seed; before it starts they pass through to libc.
//
// Covered: clock_gettime, gettimeofday, time, getrandom, getentropy, getpid,
// sysconf's CPU counts, and `syscall` for the same questions asked by number
// (OpenSSL asks for randomness that way). Not covered: reading /dev/urandom,
// and a `syscall` instruction written by hand. Those still reach the world.
//
// `malloc` is replaced for the whole process, not just while a run is active:
// a block can't be handed out by one allocator and freed by another. The
// arena sits at a fixed address and every block is filled with 0xAA, so under
// one seed every address is the same on every run and an uninitialized read
// reads the same garbage.
//
// Linux only, and only with -DRASK_SIM. Anything else compiles this file to
// nothing.

#include "rask_runtime.h"

#if defined(RASK_SIM) && defined(__linux__)

#include "sim.h"

#include <dlfcn.h>
#include <errno.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

// ─── Clock ──────────────────────────────────────────────────

static int realtime_clock(clockid_t id) {
    return id == CLOCK_REALTIME || id == CLOCK_REALTIME_COARSE ||
           id == CLOCK_REALTIME_ALARM || id == CLOCK_TAI;
}

static void ns_to_timespec(int64_t ns, struct timespec *ts) {
    ts->tv_sec = (time_t)(ns / 1000000000LL);
    ts->tv_nsec = (long)(ns % 1000000000LL);
}

static int (*real_clock_gettime(void))(clockid_t, struct timespec *) {
    static int (*fn)(clockid_t, struct timespec *);
    if (!fn) fn = (int (*)(clockid_t, struct timespec *))dlsym(RTLD_NEXT, "clock_gettime");
    return fn;
}

// Every clock is the virtual one: realtime clocks read sim's wall clock, the
// rest (monotonic, boot time, CPU time) read time since the run began. A read
// isn't a scheduling step: C can't be switched away from mid-call, since it
// may hold a lock of its own that the next task would then wait on. So C that
// spins on the clock waiting for time to pass spins forever. Time moves when
// a task yields to the runtime.
int clock_gettime(clockid_t id, struct timespec *ts) {
    if (!rask_sim_active()) return real_clock_gettime()(id, ts);
    ns_to_timespec(realtime_clock(id) ? rask_sim_wall_ns() : rask_sim_time_ns(), ts);
    return 0;
}

int gettimeofday(struct timeval *restrict tv, void *restrict tz) {
    if (!rask_sim_active()) {
        static int (*fn)(struct timeval *, void *);
        if (!fn) fn = (int (*)(struct timeval *, void *))dlsym(RTLD_NEXT, "gettimeofday");
        return fn(tv, tz);
    }
    int64_t ns = rask_sim_wall_ns();
    tv->tv_sec = (time_t)(ns / 1000000000LL);
    tv->tv_usec = (suseconds_t)(ns % 1000000000LL / 1000);
    return 0;
}

time_t time(time_t *out) {
    if (!rask_sim_active()) {
        static time_t (*fn)(time_t *);
        if (!fn) fn = (time_t (*)(time_t *))dlsym(RTLD_NEXT, "time");
        return fn(out);
    }
    time_t t = (time_t)(rask_sim_wall_ns() / 1000000000LL);
    if (out) *out = t;
    return t;
}

// ─── Random ─────────────────────────────────────────────────

// One draw from the calling task's random stream per call, stretched to the
// length asked for, so a library's appetite for randomness can't shift
// another task's draws (sim/SD3).
static void fill_random(void *buf, size_t n) {
    uint64_t state = rask_sim_random_seed();
    unsigned char *p = (unsigned char *)buf;
    while (n > 0) {
        uint64_t z = (state += 0x9e3779b97f4a7c15ULL);
        z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
        z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
        z ^= z >> 31;
        size_t k = n < sizeof z ? n : sizeof z;
        memcpy(p, &z, k);
        p += k;
        n -= k;
    }
}

ssize_t getrandom(void *buf, size_t n, unsigned int flags) {
    if (!rask_sim_active()) {
        static ssize_t (*fn)(void *, size_t, unsigned int);
        if (!fn) fn = (ssize_t (*)(void *, size_t, unsigned int))dlsym(RTLD_NEXT, "getrandom");
        return fn(buf, n, flags);
    }
    fill_random(buf, n);
    return (ssize_t)n;
}

int getentropy(void *buf, size_t n) {
    if (!rask_sim_active()) {
        static int (*fn)(void *, size_t);
        if (!fn) fn = (int (*)(void *, size_t))dlsym(RTLD_NEXT, "getentropy");
        return fn(buf, n);
    }
    if (n > 256) {
        errno = EIO;
        return -1;
    }
    fill_random(buf, n);
    return 0;
}

// ─── Process facts ──────────────────────────────────────────

#define SIM_PID 1000
#define SIM_CPUS 4

pid_t getpid(void) {
    if (!rask_sim_active()) {
        static pid_t (*fn)(void);
        if (!fn) fn = (pid_t (*)(void))dlsym(RTLD_NEXT, "getpid");
        return fn();
    }
    return SIM_PID;
}

long sysconf(int name) {
    static long (*fn)(int);
    if (!fn) fn = (long (*)(int))dlsym(RTLD_NEXT, "sysconf");
    if (rask_sim_active() && (name == _SC_NPROCESSORS_ONLN || name == _SC_NPROCESSORS_CONF)) {
        return SIM_CPUS;
    }
    return fn(name);
}

// ─── syscall(2) ─────────────────────────────────────────────

static long (*real_syscall(void))(long, ...) {
    static long (*fn)(long, ...);
    if (!fn) fn = (long (*)(long, ...))dlsym(RTLD_NEXT, "syscall");
    return fn;
}

// The same questions asked by number. Everything else passes through.
long syscall(long number, ...) {
    va_list ap;
    va_start(ap, number);
    long a[6];
    for (int i = 0; i < 6; i++) a[i] = va_arg(ap, long);
    va_end(ap);

    if (rask_sim_active()) {
        switch (number) {
        case SYS_getrandom:
            return getrandom((void *)a[0], (size_t)a[1], (unsigned int)a[2]);
        case SYS_clock_gettime:
            return clock_gettime((clockid_t)a[0], (struct timespec *)a[1]);
        case SYS_gettimeofday:
            return gettimeofday((struct timeval *)a[0], (void *)a[1]);
        case SYS_getpid:
            return SIM_PID;
#ifdef SYS_time
        case SYS_time:
            return (long)time((time_t *)a[0]);
#endif
        default:
            break;
        }
    }
    return real_syscall()(number, a[0], a[1], a[2], a[3], a[4], a[5]);
}

// ─── Allocator (sim/B8) ─────────────────────────────────────
//
// Power-of-two size classes carved from one fixed-address arena, each with a
// LIFO free list. Freed blocks are reused, never returned to the OS. Simple on
// purpose: one seed must give one sequence of addresses, and sim runs are
// tests, not workloads.
//
// Under AddressSanitizer the sanitizer owns malloc, so this half steps aside.

#ifndef __SANITIZE_ADDRESS__

#define ARENA_BASE ((uintptr_t)0x100000000000ULL)  // 16 TiB, clear of the heap and mmap areas
#define ARENA_SIZE ((size_t)1 << 36)               // 64 GiB of address space, reserved lazily
#define HEADER 16
#define MIN_BLOCK 32
#define CLASSES 32
#define POISON 0xAA
// Marks the header in front of an aligned pointer; its `offset` leads back to
// the payload it was carved from.
#define ALIGNED_MARK 0xFFFFFFFFu

typedef struct {
    uint32_t cls;     // block size is MIN_BLOCK << cls, header included
    uint32_t mark;    // ALIGNED_MARK in the header in front of an aligned pointer
    uint64_t offset;  // aligned pointer minus the payload it was carved from
} Header;

typedef struct FreeBlock { struct FreeBlock *next; } FreeBlock;

static struct {
    unsigned char *base;
    size_t used;
    FreeBlock *free[CLASSES];
    int lock;
} arena;

static void lock(void) {
    while (__atomic_test_and_set(&arena.lock, __ATOMIC_ACQUIRE)) {}
}

static void unlock(void) {
    __atomic_clear(&arena.lock, __ATOMIC_RELEASE);
}

static void die(const char *msg) {
    // No stdio: it may allocate, and the allocator is what's broken.
    ssize_t r = write(2, msg, strlen(msg));
    (void)r;
    abort();
}

static void arena_init(void) {
    void *p = mmap((void *)ARENA_BASE, ARENA_SIZE, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE | MAP_FIXED_NOREPLACE, -1, 0);
    if (p == MAP_FAILED || p != (void *)ARENA_BASE) {
        die("rask sim: couldn't reserve the fixed-address heap (sim/B8)\n");
    }
    arena.base = (unsigned char *)p;
}

static int in_arena(const void *p) {
    return arena.base && (const unsigned char *)p >= arena.base &&
           (const unsigned char *)p < arena.base + ARENA_SIZE;
}

static int class_for(size_t n) {
    size_t need = n + HEADER;
    int cls = 0;
    while (cls < CLASSES && ((size_t)MIN_BLOCK << cls) < need) cls++;
    return cls;
}

static size_t capacity(int cls) {
    return ((size_t)MIN_BLOCK << cls) - HEADER;
}

static void *alloc_locked(size_t n) {
    if (!arena.base) arena_init();
    int cls = class_for(n);
    if (cls >= CLASSES) return NULL;
    unsigned char *block;
    if (arena.free[cls]) {
        block = (unsigned char *)arena.free[cls] - HEADER;
        arena.free[cls] = arena.free[cls]->next;
    } else {
        size_t size = (size_t)MIN_BLOCK << cls;
        if (arena.used + size > ARENA_SIZE) return NULL;
        block = arena.base + arena.used;
        arena.used += size;
    }
    Header *h = (Header *)block;
    h->cls = (uint32_t)cls;
    h->mark = 0;
    h->offset = 0;
    return block + HEADER;
}

// The payload a pointer from this allocator was carved from, and how far into
// it the pointer sits.
static unsigned char *payload_of(void *p, size_t *skip) {
    Header *h = (Header *)((unsigned char *)p - HEADER);
    if (h->mark == ALIGNED_MARK) {
        *skip = h->offset;
        return (unsigned char *)p - h->offset;
    }
    *skip = 0;
    return (unsigned char *)p;
}

static size_t usable_locked(void *p) {
    size_t skip;
    unsigned char *payload = payload_of(p, &skip);
    Header *h = (Header *)(payload - HEADER);
    return capacity((int)h->cls) - skip;
}

static void free_locked(void *p) {
    size_t skip;
    unsigned char *payload = payload_of(p, &skip);
    Header *h = (Header *)(payload - HEADER);
    FreeBlock *f = (FreeBlock *)payload;
    f->next = arena.free[h->cls];
    arena.free[h->cls] = f;
}

void *malloc(size_t n) {
    lock();
    void *p = alloc_locked(n);
    unlock();
    if (!p) {
        errno = ENOMEM;
        return NULL;
    }
    memset(p, POISON, n);
    return p;
}

void free(void *p) {
    // A block from before this allocator existed (the dynamic loader's own
    // early allocations) isn't ours to reuse. Leaking it is harmless.
    if (!p || !in_arena(p)) return;
    lock();
    free_locked(p);
    unlock();
}

void *calloc(size_t count, size_t size) {
    if (size && count > SIZE_MAX / size) {
        errno = ENOMEM;
        return NULL;
    }
    size_t n = count * size;
    lock();
    void *p = alloc_locked(n);
    unlock();
    if (!p) {
        errno = ENOMEM;
        return NULL;
    }
    memset(p, 0, n);
    return p;
}

void *realloc(void *p, size_t n) {
    if (!p) return malloc(n);
    if (!in_arena(p)) die("rask sim: realloc of a block the sim heap didn't hand out\n");
    if (n == 0) {
        free(p);
        return NULL;
    }
    lock();
    size_t have = usable_locked(p);
    unlock();
    if (n <= have) return p;
    void *q = malloc(n);
    if (!q) return NULL;
    memcpy(q, p, have);
    free(p);
    return q;
}

void *reallocarray(void *p, size_t count, size_t size) {
    if (size && count > SIZE_MAX / size) {
        errno = ENOMEM;
        return NULL;
    }
    return realloc(p, count * size);
}

static void *aligned(size_t align, size_t n) {
    if (align <= HEADER) return malloc(n);
    lock();
    unsigned char *payload = (unsigned char *)alloc_locked(n + align + HEADER);
    unlock();
    if (!payload) return NULL;
    uintptr_t at = ((uintptr_t)payload + HEADER + align - 1) & ~(uintptr_t)(align - 1);
    Header *h = (Header *)(at - HEADER);
    h->cls = 0;
    h->mark = ALIGNED_MARK;
    h->offset = at - (uintptr_t)payload;
    memset((void *)at, POISON, n);
    return (void *)at;
}

int posix_memalign(void **out, size_t align, size_t n) {
    if (align < sizeof(void *) || (align & (align - 1))) return EINVAL;
    void *p = aligned(align, n);
    if (!p) return ENOMEM;
    *out = p;
    return 0;
}

void *aligned_alloc(size_t align, size_t n) {
    if (align == 0 || (align & (align - 1))) {
        errno = EINVAL;
        return NULL;
    }
    return aligned(align, n);
}

void *memalign(size_t align, size_t n) {
    return aligned_alloc(align, n);
}

void *valloc(size_t n) {
    return aligned((size_t)getpagesize(), n);
}

void *pvalloc(size_t n) {
    size_t page = (size_t)getpagesize();
    return aligned(page, (n + page - 1) & ~(page - 1));
}

size_t malloc_usable_size(void *p) {
    if (!p || !in_arena(p)) return 0;
    lock();
    size_t n = usable_locked(p);
    unlock();
    return n;
}

#endif // __SANITIZE_ADDRESS__

#endif // RASK_SIM && __linux__
