// SPDX-License-Identifier: (MIT OR Apache-2.0)

// The filesystem under sim (sim/B5): reads fall through to the real tree,
// writes land in memory and vanish with the test's process.
//
// The overlay maps a path to what the test has made of it — a file with its
// bytes, a directory, or gone. A path it has no entry for is whatever the real
// tree says, read-only. A file opened from the overlay is a real `FILE *` whose
// reads and writes land in the entry's buffer, so fread/fwrite/fseek, `File`
// handles and everything built on them work unchanged.
//
// Paths are compared as spelled, after dropping a leading "./" and any trailing
// slash. `a/../b` and `b` are two paths here; nothing in the tests that exist
// needs more.
//
// Built only with -DRASK_SIM.

#include "rask_runtime.h"

#ifdef RASK_SIM

#include "sim.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <dirent.h>

typedef enum { ENT_FILE, ENT_DIR, ENT_GONE } EntKind;

typedef struct Ent {
    char       *path;
    EntKind     kind;
    char       *data;
    size_t      len;
    size_t      cap;
    int64_t     mtime_ns;   // virtual, like every clock under sim
    struct Ent *next;
} Ent;

static Ent *ents;

static void *xalloc(size_t n) {
    void *p = calloc(1, n ? n : 1);
    if (!p) {
        fprintf(stderr, "sim: out of memory in the filesystem overlay\n");
        abort();
    }
    return p;
}

static char *norm(const char *p) {
    while (p[0] == '.' && p[1] == '/') {
        p += 2;
        while (*p == '/') p++;
    }
    size_t n = strlen(p);
    while (n > 1 && p[n - 1] == '/') n--;
    char *out = (char *)xalloc(n + 1);
    memcpy(out, p, n);
    out[n] = '\0';
    return out;
}

// "a/b/c" → "a/b"; "c" → "."; "/c" → "/".
static char *parent_of(const char *np) {
    const char *slash = strrchr(np, '/');
    if (!slash) return norm(".");
    if (slash == np) return norm("/");
    size_t n = (size_t)(slash - np);
    char *out = (char *)xalloc(n + 1);
    memcpy(out, np, n);
    return out;
}

static Ent *find(const char *np) {
    for (Ent *e = ents; e; e = e->next) {
        if (strcmp(e->path, np) == 0) return e;
    }
    return NULL;
}

// A directory the test removed or renamed away takes its whole subtree with it.
static int ancestor_gone(const char *np) {
    size_t n = strlen(np);
    for (Ent *e = ents; e; e = e->next) {
        if (e->kind != ENT_GONE) continue;
        size_t k = strlen(e->path);
        if (k < n && strncmp(np, e->path, k) == 0 && np[k] == '/') return 1;
    }
    return 0;
}

static Ent *put(const char *np, EntKind kind) {
    Ent *e = find(np);
    if (!e) {
        e = (Ent *)xalloc(sizeof(Ent));
        e->path = norm(np);
        e->next = ents;
        ents = e;
    }
    if (kind != e->kind) e->len = 0;
    e->kind = kind;
    e->mtime_ns = rask_sim_time_ns();
    return e;
}

static void reserve(Ent *e, size_t need) {
    if (need <= e->cap) return;
    size_t cap = e->cap ? e->cap : 64;
    while (cap < need) cap *= 2;
    e->data = (char *)realloc(e->data, cap);
    if (!e->data) {
        fprintf(stderr, "sim: out of memory in the filesystem overlay\n");
        abort();
    }
    e->cap = cap;
}

// 0 nothing there, 1 a file, 2 a directory — the overlay first, then the tree.
static int kind_of(const char *np) {
    Ent *e = find(np);
    if (e) return e->kind == ENT_FILE ? 1 : e->kind == ENT_DIR ? 2 : 0;
    if (ancestor_gone(np)) return 0;
    struct stat st;
    if (stat(np, &st) != 0) return 0;
    return S_ISDIR(st.st_mode) ? 2 : 1;
}

static int parent_is_dir(const char *np) {
    char *parent = parent_of(np);
    int ok = kind_of(parent) == 2;
    free(parent);
    return ok;
}

// Bring a real file's bytes into the overlay before the test changes it.
static int import_real(Ent *e, const char *np) {
    FILE *f = fopen(np, "rb");
    if (!f) return -1;
    char buf[8192];
    size_t n;
    e->len = 0;
    while ((n = fread(buf, 1, sizeof(buf), f)) > 0) {
        reserve(e, e->len + n);
        memcpy(e->data + e->len, buf, n);
        e->len += n;
    }
    fclose(f);
    return 0;
}

// ─── Streams over an entry ──────────────────────────────────

typedef struct {
    Ent   *e;
    size_t pos;
    int    append;
} Cookie;

static ssize_t cookie_read(void *c, char *buf, size_t size) {
    Cookie *k = (Cookie *)c;
    if (k->pos >= k->e->len) return 0;
    size_t n = k->e->len - k->pos;
    if (n > size) n = size;
    memcpy(buf, k->e->data + k->pos, n);
    k->pos += n;
    return (ssize_t)n;
}

static ssize_t cookie_write(void *c, const char *buf, size_t size) {
    Cookie *k = (Cookie *)c;
    Ent *e = k->e;
    if (k->append) k->pos = e->len;
    reserve(e, k->pos + size);
    if (k->pos > e->len) memset(e->data + e->len, 0, k->pos - e->len);
    memcpy(e->data + k->pos, buf, size);
    k->pos += size;
    if (k->pos > e->len) e->len = k->pos;
    e->mtime_ns = rask_sim_time_ns();
    return (ssize_t)size;
}

static int cookie_seek_to(Cookie *k, int64_t off, int whence, int64_t *out) {
    int64_t base = whence == SEEK_SET ? 0
                 : whence == SEEK_CUR ? (int64_t)k->pos
                 : (int64_t)k->e->len;
    int64_t pos = base + off;
    if (pos < 0) {
        errno = EINVAL;
        return -1;
    }
    k->pos = (size_t)pos;
    *out = pos;
    return 0;
}

static int cookie_close(void *c) {
    free(c);
    return 0;
}

#ifdef __APPLE__
static int apple_read(void *c, char *buf, int n) { return (int)cookie_read(c, buf, (size_t)n); }
static int apple_write(void *c, const char *buf, int n) { return (int)cookie_write(c, buf, (size_t)n); }
static fpos_t apple_seek(void *c, fpos_t off, int whence) {
    int64_t out;
    if (cookie_seek_to((Cookie *)c, (int64_t)off, whence, &out) != 0) return -1;
    return (fpos_t)out;
}
#else
static int linux_seek(void *c, off64_t *off, int whence) {
    int64_t out;
    if (cookie_seek_to((Cookie *)c, (int64_t)*off, whence, &out) != 0) return -1;
    *off = (off64_t)out;
    return 0;
}
#endif

static FILE *open_stream(Ent *e, const char *mode, int append) {
    Cookie *k = (Cookie *)xalloc(sizeof(Cookie));
    k->e = e;
    k->append = append;
#ifdef __APPLE__
    FILE *f = funopen(k, apple_read, apple_write, apple_seek, cookie_close);
    (void)mode;
#else
    cookie_io_functions_t io = {
        .read = cookie_read,
        .write = cookie_write,
        .seek = linux_seek,
        .close = cookie_close,
    };
    FILE *f = fopencookie(k, mode, io);
#endif
    if (!f) free(k);
    return f;
}

// ─── The calls runtime.c routes here ────────────────────────

FILE *rask_sim_fs_fopen(const char *path, const char *mode, int *handled) {
    char *np = norm(path);
    int plus = strchr(mode, '+') != NULL;
    Ent *e = find(np);
    FILE *f = NULL;
    *handled = 1;

    switch (mode[0]) {
    case 'r':
        if (e && e->kind == ENT_FILE) {
            f = open_stream(e, mode, 0);
        } else if (e || ancestor_gone(np)) {
            errno = e && e->kind == ENT_DIR ? EISDIR : ENOENT;
        } else if (!plus) {
            *handled = 0;   // untouched: the real tree answers a plain read
        } else if (kind_of(np) == 1) {
            e = put(np, ENT_FILE);
            import_real(e, np);
            f = open_stream(e, mode, 0);
        } else {
            errno = ENOENT;
        }
        break;
    case 'w':
        if (kind_of(np) == 2) {
            errno = EISDIR;
        } else if (!parent_is_dir(np)) {
            errno = ENOENT;
        } else {
            e = put(np, ENT_FILE);
            e->len = 0;
            f = open_stream(e, mode, 0);
        }
        break;
    case 'a': {
        int kind = kind_of(np);
        if (kind == 2) {
            errno = EISDIR;
        } else if (kind == 0 && !parent_is_dir(np)) {
            errno = ENOENT;
        } else {
            int fresh = !(e && e->kind == ENT_FILE);
            e = put(np, ENT_FILE);
            if (fresh && kind == 1) import_real(e, np);
            f = open_stream(e, mode, 1);
        }
        break;
    }
    default:
        errno = EINVAL;
        break;
    }
    free(np);
    return f;
}

int rask_sim_fs_rename(const char *from, const char *to) {
    char *nf = norm(from);
    char *nt = norm(to);
    int kind = kind_of(nf);
    int rc = -1;
    if (kind == 0) {
        errno = ENOENT;
    } else if (kind == 2) {
        rask_sim_unsimulated("`fs.rename(\"%s\", \"%s\")` on a directory", from, to);
    } else if (!parent_is_dir(nt)) {
        errno = ENOENT;
    } else if (kind_of(nt) == 2) {
        errno = EISDIR;
    } else if (strcmp(nf, nt) == 0) {
        rc = 0;
    } else {
        Ent *src = find(nf);
        Ent *dst = put(nt, ENT_FILE);
        if (src && src->kind == ENT_FILE) {
            reserve(dst, src->len);
            if (src->len) memcpy(dst->data, src->data, src->len);
            dst->len = src->len;
        } else {
            import_real(dst, nf);
        }
        put(nf, ENT_GONE);
        rc = 0;
    }
    free(nf);
    free(nt);
    return rc;
}

int rask_sim_fs_remove(const char *path) {
    char *np = norm(path);
    int kind = kind_of(np);
    int rc = -1;
    if (kind == 0) {
        errno = ENOENT;
    } else if (kind == 2) {
        // Removing a directory needs to know it is empty, which means merging
        // the overlay with a real listing. Not modelled yet.
        rask_sim_unsimulated("`fs.remove(\"%s\")` on a directory", path);
    } else {
        put(np, ENT_GONE);
        rc = 0;
    }
    free(np);
    return rc;
}

int rask_sim_fs_mkdir(const char *path) {
    char *np = norm(path);
    int rc = -1;
    if (kind_of(np) != 0) {
        errno = EEXIST;
    } else if (!parent_is_dir(np)) {
        errno = ENOENT;
    } else {
        put(np, ENT_DIR);
        rc = 0;
    }
    free(np);
    return rc;
}

int rask_sim_fs_stat(const char *path, struct stat *st) {
    char *np = norm(path);
    Ent *e = find(np);
    int rc = SIM_FS_PASS;
    if ((e && e->kind == ENT_GONE) || (!e && ancestor_gone(np))) {
        errno = ENOENT;
        rc = -1;
    } else if (e) {
        memset(st, 0, sizeof(*st));
        st->st_mode = e->kind == ENT_DIR ? (S_IFDIR | 0755) : (S_IFREG | 0644);
        st->st_size = (off_t)e->len;
        time_t secs = (time_t)(1577836800LL + e->mtime_ns / 1000000000LL);
        st->st_mtime = secs;
        st->st_atime = secs;
        rc = 0;
    }
    free(np);
    return rc;
}

static int name_cmp(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

// What the test would see in `path`: the real entries it hasn't removed plus
// the ones it made, sorted. Sorted because readdir's order is the filesystem's,
// which differs between machines and so can't be part of a replay.
//
// NULL with errno set when there is no such directory.
char **rask_sim_fs_list(const char *path, size_t *count) {
    char *np = norm(path);
    size_t n = 0, cap = 16;
    char **names = NULL;

    if (kind_of(np) != 2) {
        errno = kind_of(np) == 1 ? ENOTDIR : ENOENT;
        free(np);
        return NULL;
    }
    names = (char **)xalloc(cap * sizeof(char *));

#define PUSH(name) do { \
        if (n == cap) { \
            cap *= 2; \
            names = (char **)realloc(names, cap * sizeof(char *)); \
            if (!names) abort(); \
        } \
        names[n++] = strdup(name); \
    } while (0)

    // The real directory, unless this test made it (then there is none).
    Ent *self = find(np);
    if (!self) {
        DIR *d = opendir(np);
        if (d) {
            struct dirent *de;
            while ((de = readdir(d)) != NULL) {
                if (strcmp(de->d_name, ".") == 0 || strcmp(de->d_name, "..") == 0) continue;
                size_t len = strlen(np) + 1 + strlen(de->d_name) + 1;
                char *joined = (char *)xalloc(len);
                snprintf(joined, len, "%s/%s", np, de->d_name);
                char *child = norm(joined);
                Ent *e = find(child);
                if (!e) PUSH(de->d_name);   // an overlay entry is listed below
                free(child);
                free(joined);
            }
            closedir(d);
        }
    }
    for (Ent *e = ents; e; e = e->next) {
        if (e->kind == ENT_GONE) continue;
        char *parent = parent_of(e->path);
        if (strcmp(parent, np) == 0) {
            const char *slash = strrchr(e->path, '/');
            PUSH(slash ? slash + 1 : e->path);
        }
        free(parent);
    }
#undef PUSH

    qsort(names, n, sizeof(char *), name_cmp);
    free(np);
    *count = n;
    return names;
}

#endif // RASK_SIM
