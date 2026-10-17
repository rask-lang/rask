// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// The sim heap (compiler/runtime/sim_libc.c, sim/B8) on its own. Every sim
// run allocates through it, so a bug here breaks every sim test at once.
//
// Linked with sim_libc.c, this binary's every allocation goes through that
// allocator, stdio's included. The checks:
//   - 100k random malloc/calloc/realloc/aligned/free calls against a model:
//     each live block holds a pattern unique to it, checked whole on every
//     realloc and free, so an overlap or a short copy shows as a wrong byte
//   - fresh malloc bytes are 0xAA, calloc bytes are 0, freed bytes are 0xAA
//   - usable size covers the request, alignment holds
//   - the same calls give the same addresses in two processes
//   - requests too big to fit fail with ENOMEM instead of wrapping
//   - a double free and a wild free abort
//   - a core dump would cover the heap in use, not the whole 64 GiB
//     reservation: a CI runner pipes cores to a collector that reads every
//     byte, and one aborted test held the job for minutes
//
// sim_libc.c answers from the seed only while a sim run is active. Here none
// is, so the stubs below say so and the clock and random calls pass through.

#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

int rask_sim_active(void) { return 0; }
int64_t rask_sim_time_ns(void) { return 0; }
int64_t rask_sim_wall_ns(void) { return 0; }
uint64_t rask_sim_random_seed(void) { return 0; }

void *memalign(size_t align, size_t n);
size_t malloc_usable_size(void *p);

#define SLOTS 2048
#define OPS 100000
#define ARENA_BASE ((uintptr_t)0x100000000000ULL)

static int failures;

#define CHECK(cond, ...)                                   \
    do {                                                   \
        if (!(cond)) {                                     \
            fprintf(stderr, "heap_check: " __VA_ARGS__);   \
            fprintf(stderr, " (line %d)\n", __LINE__);     \
            failures++;                                    \
            if (failures > 20) exit(1);                    \
        }                                                  \
    } while (0)

static uint64_t rng = 0x9e3779b97f4a7c15ULL;
static uint64_t next(void) {
    rng ^= rng << 13;
    rng ^= rng >> 7;
    rng ^= rng << 17;
    return rng;
}

// Mostly small, sometimes up to 256 KB, now and then zero.
static size_t pick_size(void) {
    uint64_t r = next() % 100;
    if (r < 5) return 0;
    if (r < 70) return next() % 128;
    if (r < 95) return next() % 8192;
    return next() % (1 << 18);
}

typedef struct {
    unsigned char *p;
    size_t n;
    unsigned char tag;
    size_t align;
} Slot;

static Slot slots[SLOTS];

static void fill(Slot *s) {
    for (size_t i = 0; i < s->n; i++) s->p[i] = (unsigned char)(s->tag + i * 7);
}

static void verify(const Slot *s, size_t upto, const char *when) {
    for (size_t i = 0; i < upto; i++) {
        if (s->p[i] != (unsigned char)(s->tag + i * 7)) {
            CHECK(0, "block %p (%zu bytes) byte %zu changed before %s", (void *)s->p, s->n, i, when);
            return;
        }
    }
}

static void check_fresh(const unsigned char *p, size_t n, unsigned char want, const char *what) {
    for (size_t i = 0; i < n; i++) {
        if (p[i] != want) {
            CHECK(0, "%s byte %zu is 0x%02x, not 0x%02x", what, i, p[i], want);
            return;
        }
    }
}

static void check_block(const Slot *s, const char *what) {
    CHECK((uintptr_t)s->p >= ARENA_BASE, "%s %p is outside the fixed arena", what, (void *)s->p);
    CHECK(malloc_usable_size(s->p) >= s->n, "%s: usable %zu < requested %zu", what,
          malloc_usable_size(s->p), s->n);
    size_t align = s->align ? s->align : 16;
    CHECK((uintptr_t)s->p % align == 0, "%s %p isn't %zu-aligned", what, (void *)s->p, align);
}

// The random workload. Returns a hash of every address handed out, so two
// processes can compare.
static uint64_t workload(int checking) {
    uint64_t hash = 1469598103934665603ULL;
    for (int op = 0; op < OPS; op++) {
        Slot *s = &slots[next() % SLOTS];
        uint64_t kind = next() % 100;
        if (s->p == NULL) {
            size_t n = pick_size();
            s->tag = (unsigned char)next();
            s->align = 0;
            if (kind < 60) {
                s->p = malloc(n);
                s->n = n;
                if (checking) check_fresh(s->p, n, 0xAA, "fresh malloc");
            } else if (kind < 80) {
                size_t count = n / 8 + 1;
                s->p = calloc(count, 8);
                s->n = count * 8;
                if (checking) check_fresh(s->p, s->n, 0, "calloc");
            } else {
                s->align = (size_t)32 << (next() % 8);   // 32 .. 4096
                switch (next() % 3) {
                case 0:
                    CHECK(posix_memalign((void **)&s->p, s->align, n) == 0, "posix_memalign failed");
                    break;
                case 1: s->p = aligned_alloc(s->align, n); break;
                default: s->p = memalign(s->align, n); break;
                }
                s->n = n;
                if (checking) check_fresh(s->p, n, 0xAA, "fresh aligned");
            }
            CHECK(s->p != NULL, "allocation of %zu bytes failed", n);
            if (checking) check_block(s, "allocation");
            fill(s);
        } else if (kind < 40) {
            size_t n = pick_size();
            size_t keep = n < s->n ? n : s->n;
            if (checking) verify(s, s->n, "realloc");
            unsigned char *q = realloc(s->p, n);
            if (n == 0) {
                // realloc(p, 0) frees here, as glibc's does.
                CHECK(q == NULL, "realloc(p, 0) returned %p", (void *)q);
                s->p = NULL;
                continue;
            }
            CHECK(q != NULL, "realloc to %zu failed", n);
            s->p = q;
            s->align = 0;
            size_t old = s->n;
            s->n = keep;
            if (checking) {
                verify(s, keep, "after realloc");
                check_block(s, "realloc");
            }
            // Grow the model to the new size and refill past what was kept.
            s->n = n;
            for (size_t i = keep; i < n; i++) s->p[i] = (unsigned char)(s->tag + i * 7);
            (void)old;
        } else {
            if (checking) verify(s, s->n, "free");
            unsigned char *p = s->p;
            size_t n = s->n;
            free(p);
            // The block is poisoned at once: a read after free sees 0xAA past
            // the free list's link in its first 8 bytes.
            if (checking && n > 8) check_fresh(p + 8, n - 8, 0xAA, "freed block");
            s->p = NULL;
        }
        hash = (hash ^ (uint64_t)(uintptr_t)s->p) * 1099511628211ULL;
    }
    for (int i = 0; i < SLOTS; i++) {
        if (slots[i].p) {
            if (checking) verify(&slots[i], slots[i].n, "final free");
            free(slots[i].p);
            slots[i].p = NULL;
        }
    }
    return hash;
}

// The workload's address hash, from a fresh process.
static uint64_t hash_in_child(void) {
    int fds[2];
    if (pipe(fds) != 0) return 0;
    pid_t pid = fork();
    if (pid == 0) {
        close(fds[0]);
        rng = 12345;
        uint64_t h = workload(0);
        ssize_t w = write(fds[1], &h, sizeof h);
        _exit(w == sizeof h ? 0 : 1);
    }
    close(fds[1]);
    uint64_t h = 0;
    ssize_t r = read(fds[0], &h, sizeof h);
    close(fds[0]);
    int status;
    waitpid(pid, &status, 0);
    return r == sizeof h ? h : 0;
}

// Does `f` abort in a child?
static int aborts(void (*f)(void)) {
    pid_t pid = fork();
    if (pid == 0) {
        // The death message is expected; keep the gate's output clean, and
        // don't leave a core behind for an abort we asked for.
        if (!freopen("/dev/null", "w", stderr)) _exit(2);
        struct rlimit none = {0, 0};
        setrlimit(RLIMIT_CORE, &none);
        f();
        _exit(0);
    }
    int status;
    waitpid(pid, &status, 0);
    return WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT;
}

// Through a volatile, so the compiler doesn't refuse the double free it's
// here to test.
static void *volatile again;

static void double_free(void) {
    again = malloc(40);
    free(again);
    free(again);
}

static void double_free_aligned(void) {
    again = aligned_alloc(256, 100);
    free(again);
    free(again);
}

static void wild_free(void) {
    char *p = malloc(64);
    free(p + 24);
}

// Bytes of the arena a core dump would include: its mappings in
// /proc/self/smaps without the "dd" (don't dump) flag.
static unsigned long long dumpable_arena_bytes(void) {
    FILE *f = fopen("/proc/self/smaps", "r");
    if (!f) return 0;
    char line[512];
    unsigned long long total = 0, lo = 0, hi = 0;
    int in_arena = 0;
    while (fgets(line, sizeof line, f)) {
        unsigned long long a, b;
        if (sscanf(line, "%llx-%llx ", &a, &b) == 2 && strchr(line, '-') < strchr(line, ' ')) {
            lo = a;
            hi = b;
            in_arena = a >= ARENA_BASE && a < ARENA_BASE + (1ULL << 36);
        } else if (in_arena && strncmp(line, "VmFlags:", 8) == 0 && !strstr(line, " dd")) {
            total += hi - lo;
        }
    }
    fclose(f);
    return total;
}

int main(void) {
    workload(1);

    uint64_t a = hash_in_child();
    uint64_t b = hash_in_child();
    CHECK(a != 0 && a == b, "two processes got different addresses: %llx vs %llx",
          (unsigned long long)a, (unsigned long long)b);

    // Volatile, so the compiler can't decide these calls fail on its own.
    volatile size_t huge = SIZE_MAX;
    volatile size_t three = 3;
    errno = 0;
    CHECK(malloc(huge) == NULL && errno == ENOMEM, "malloc(SIZE_MAX) didn't fail with ENOMEM");
    CHECK(malloc(huge - 8) == NULL, "malloc(SIZE_MAX - 8) wrapped to a small block");
    CHECK(calloc(huge / 2, 4) == NULL, "calloc overflow wasn't caught");
    CHECK(aligned_alloc(4096, huge - 100) == NULL, "aligned_alloc wrapped to a small block");
    CHECK(aligned_alloc(three, 16) == NULL && errno == EINVAL, "aligned_alloc took alignment 3");
    void *z = malloc(0);
    CHECK(z != NULL, "malloc(0) returned NULL");
    free(z);
    free(NULL);

    CHECK(aborts(double_free), "a double free didn't abort");
    CHECK(aborts(double_free_aligned), "a double free of an aligned block didn't abort");
    CHECK(aborts(wild_free), "freeing a pointer into the middle of a block didn't abort");

    unsigned long long dumpable = dumpable_arena_bytes();
    CHECK(dumpable > 0 && dumpable <= (1ULL << 30),
          "a core dump would include %llu bytes of the heap's reservation", dumpable);

    if (failures) return 1;
    printf("heap_check: ok\n");
    return 0;
}
