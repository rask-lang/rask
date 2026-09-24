// SPDX-License-Identifier: (MIT OR Apache-2.0)

// The filesystem under sim (sim/B5): reads see the real tree, writes land in
// memory and vanish with the test's process.
//
// Two tables, the way a real filesystem has them. A `Node` is a file's
// contents, what an inode is: the bytes, their mtime, whether the seed made
// the file sick. A `Name` maps a path to a node, a directory, or "gone". A
// stream holds its node, not its path, so renaming or removing a file under
// an open stream behaves the POSIX way: the stream keeps the file.
//
// A real file the test only reads is never copied: its node points at the
// real path and reads go there. The first write copies the bytes in, so a
// test can't change the real tree, and reading `/proc` or a large file costs
// what it costs outside sim.
//
// Paths are canonical before anything looks at them: absolute, with `.`, `..`
// and repeated slashes resolved, so `d/./x`, `d//x`, `$PWD/d/x` and
// `d/e/../x` are one file. `..` is resolved on the spelling, not through
// symlinks. Reports name a file by the path the test first opened it as.
//
// Every operation is a scheduling point and takes a seeded latency (sim/S3,
// C4), so two tasks writing one file interleave the way the seed says. A
// stream that can write is unbuffered, so each `fwrite` reaches the overlay
// as one operation and an injected fault has no partial effect (sim/F5).
//
// Built only with -DRASK_SIM.

#include "rask_runtime.h"

#ifdef RASK_SIM

#include "sim.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define MAX_FS_LATENCY_NS 100000   // 0.1 ms

typedef struct Node {
    char   *data;
    size_t  len, cap;
    char   *backing;     // the real file still holding the bytes; NULL once copied in
    int64_t mtime_ns;    // virtual; only meaningful once copied in
    int     sick;        // sim/F4, drawn on the first open with IoError on
    int     sick_drawn;
    char   *label;       // the path as the test first spelled it
    int     refs;        // names and streams reaching it
} Node;

typedef enum { NAME_FILE, NAME_DIR, NAME_GONE } NameKind;

typedef struct Name {
    char        *path;   // canonical
    NameKind     kind;
    Node        *node;   // NAME_FILE only
    struct Name *next;
} Name;

static Name *names;

static void *xalloc(size_t n) {
    void *p = calloc(1, n ? n : 1);
    if (!p) {
        fprintf(stderr, "sim: out of memory in the filesystem overlay\n");
        abort();
    }
    return p;
}

static char *xstrdup(const char *s) {
    size_t n = strlen(s);
    char *out = (char *)xalloc(n + 1);
    memcpy(out, s, n + 1);
    return out;
}

// Each operation is a scheduling point that takes a moment (sim/S3, C4). The
// effect happens after the wait, at one step, so nothing another task does
// lands in the middle of it.
static void fs_step(void) {
    rask_sim_sleep((int64_t)(rask_sim_fault_draw() % MAX_FS_LATENCY_NS));
}

// ─── Paths ──────────────────────────────────────────────────

static const char *cwd(void) {
    static char *dir;
    if (!dir) {
        char buf[4096];
        dir = xstrdup(getcwd(buf, sizeof(buf)) ? buf : "/");
    }
    return dir;
}

// Absolute, with ".", ".." and empty components gone. "" stays "", which
// names nothing.
static char *canon(const char *path) {
    if (!path[0]) return xstrdup("");
    const char *base = path[0] == '/' ? "" : cwd();
    size_t cap = strlen(base) + strlen(path) + 2;
    char *out = (char *)xalloc(cap);
    size_t len = 0;
    for (int pass = 0; pass < 2; pass++) {
        const char *p = pass == 0 ? base : path;
        while (*p) {
            while (*p == '/') p++;
            const char *start = p;
            while (*p && *p != '/') p++;
            size_t n = (size_t)(p - start);
            if (n == 0 || (n == 1 && start[0] == '.')) continue;
            if (n == 2 && start[0] == '.' && start[1] == '.') {
                while (len > 0 && out[len - 1] != '/') len--;
                if (len > 0) len--;
                continue;
            }
            out[len++] = '/';
            memcpy(out + len, start, n);
            len += n;
        }
    }
    if (len == 0) out[len++] = '/';
    out[len] = '\0';
    return out;
}

// "/a/b" → "/a"; "/a" → "/"; "/" → "/".
static char *parent_of(const char *np) {
    const char *slash = strrchr(np, '/');
    size_t n = slash && slash != np ? (size_t)(slash - np) : 1;
    char *out = (char *)xalloc(n + 1);
    memcpy(out, np, n);
    return out;
}

// ─── The two tables ─────────────────────────────────────────

static Node *node_new(const char *label, const char *backing) {
    Node *n = (Node *)xalloc(sizeof(Node));
    n->label = xstrdup(label);
    n->backing = backing ? xstrdup(backing) : NULL;
    n->mtime_ns = rask_sim_time_ns();
    return n;
}

static void node_release(Node *n) {
    if (!n || --n->refs > 0) return;
    free(n->data);
    free(n->backing);
    free(n->label);
    free(n);
}

static Name *find(const char *np) {
    for (Name *e = names; e; e = e->next) {
        if (strcmp(e->path, np) == 0) return e;
    }
    return NULL;
}

// A directory the test removed or renamed away takes its whole subtree with it.
static int ancestor_gone(const char *np) {
    size_t n = strlen(np);
    for (Name *e = names; e; e = e->next) {
        if (e->kind != NAME_GONE) continue;
        size_t k = strlen(e->path);
        if (k < n && strncmp(np, e->path, k) == 0 && np[k] == '/') return 1;
    }
    return 0;
}

// Point `np` at `kind` (and, for a file, `node`, whose reference it takes).
static Name *name_set(const char *np, NameKind kind, Node *node) {
    Name *e = find(np);
    if (!e) {
        e = (Name *)xalloc(sizeof(Name));
        e->path = xstrdup(np);
        e->next = names;
        names = e;
    }
    Node *old = e->node;
    e->kind = kind;
    e->node = node;
    if (node) node->refs++;
    node_release(old);
    return e;
}

typedef enum { KIND_NONE, KIND_FILE, KIND_DIR, KIND_OTHER } Kind;

// What `np` is: the overlay first, then the real tree.
static Kind kind_of(const char *np) {
    if (!np[0]) return KIND_NONE;
    Name *e = find(np);
    if (e) return e->kind == NAME_FILE ? KIND_FILE : e->kind == NAME_DIR ? KIND_DIR : KIND_NONE;
    if (ancestor_gone(np)) return KIND_NONE;
    struct stat st;
    if (stat(np, &st) != 0) return KIND_NONE;
    return S_ISREG(st.st_mode) ? KIND_FILE : S_ISDIR(st.st_mode) ? KIND_DIR : KIND_OTHER;
}

static int parent_is_dir(const char *np) {
    char *parent = parent_of(np);
    int ok = kind_of(parent) == KIND_DIR;
    free(parent);
    return ok;
}

// The node behind an existing file, giving an untouched real file one that
// reads from the real path. NULL with errno set when `np` isn't a file.
static Node *file_node(const char *np, const char *spelled) {
    Name *e = find(np);
    if (e && e->kind == NAME_FILE) return e->node;
    switch (kind_of(np)) {
    case KIND_FILE: {
        Node *n = node_new(spelled, np);
        name_set(np, NAME_FILE, n);
        return n;
    }
    case KIND_DIR:
        errno = EISDIR;
        return NULL;
    case KIND_OTHER:
        // A device, a pipe or a socket in the tree answers from outside the
        // test: /dev/urandom would make the run depend on the machine.
        rask_sim_unsimulated("`%s`, which is not a regular file", spelled);
        errno = EINVAL;
        return NULL;
    default:
        errno = ENOENT;
        return NULL;
    }
}

// Bring a real file's bytes into its node, before the first change to it.
static int node_own(Node *n) {
    if (!n->backing) return 0;
    FILE *f = fopen(n->backing, "rb");
    if (!f) return -1;
    char buf[8192];
    size_t got;
    n->len = 0;
    while ((got = fread(buf, 1, sizeof(buf), f)) > 0) {
        if (n->len + got > n->cap) {
            size_t cap = n->cap ? n->cap : 4096;
            while (cap < n->len + got) cap *= 2;
            n->data = (char *)realloc(n->data, cap);
            if (!n->data) abort();
            n->cap = cap;
        }
        memcpy(n->data + n->len, buf, got);
        n->len += got;
    }
    int failed = ferror(f);
    int err = errno;
    fclose(f);
    if (failed) {
        errno = err;
        return -1;
    }
    free(n->backing);
    n->backing = NULL;
    n->mtime_ns = rask_sim_time_ns();
    return 0;
}

static void node_truncate(Node *n) {
    free(n->backing);
    n->backing = NULL;
    n->len = 0;
    n->mtime_ns = rask_sim_time_ns();
}

static int64_t node_size(Node *n) {
    if (!n->backing) return (int64_t)n->len;
    struct stat st;
    return stat(n->backing, &st) == 0 ? (int64_t)st.st_size : 0;
}

// sim/F4: the file is the resource, so every open of it agrees.
static int node_sick(Node *n) {
    if (!rask_sim_fault_enabled(SIM_FAULT_IO_ERROR)) return 0;
    if (!n->sick_drawn) {
        char what[600];
        snprintf(what, sizeof(what), "file `%s`", n->label);
        n->sick = rask_sim_draw_sick(SIM_FAULT_IO_ERROR, what);
        n->sick_drawn = 1;
    }
    return n->sick;
}

// ─── Streams over a node ────────────────────────────────────

typedef struct {
    Node  *node;
    size_t pos;
    int    append;
    int    sick;
    int    broken;     // an injected fault landed; the stream stays failed
    int    real_fd;    // the backing file, opened on the first read of it
} Cookie;

// sim/F5: an injected error means the operation had no effect.
//
// A stream that failed stays failed, as one hitting a real device error
// does. It also has to: when a write fails, glibc retries the rest one byte
// at a time, and letting those through landed part of a write the caller
// was told had failed.
static int injected(Cookie *k, const char *op) {
    if (k->broken) {
        errno = EIO;
        return 1;
    }
    if (!k->sick) return 0;
    char what[600];
    snprintf(what, sizeof(what), "%s failed on `%s`", op, k->node->label);
    if (!rask_sim_draw_failure(what)) return 0;
    k->broken = 1;
    errno = EIO;
    return 1;
}

static ssize_t cookie_read(void *c, char *buf, size_t size) {
    Cookie *k = (Cookie *)c;
    fs_step();
    if (injected(k, "read")) return -1;
    Node *n = k->node;
    if (n->backing) {
        if (k->real_fd < 0) {
            k->real_fd = open(n->backing, O_RDONLY);
            if (k->real_fd < 0) return -1;
        }
        ssize_t got = pread(k->real_fd, buf, size, (off_t)k->pos);
        if (got > 0) k->pos += (size_t)got;
        return got;
    }
    if (k->pos >= n->len) return 0;
    size_t got = n->len - k->pos;
    if (got > size) got = size;
    memcpy(buf, n->data + k->pos, got);
    k->pos += got;
    return (ssize_t)got;
}

static ssize_t cookie_write(void *c, const char *buf, size_t size) {
    Cookie *k = (Cookie *)c;
    fs_step();
    if (injected(k, "write")) return -1;
    Node *n = k->node;
    if (node_own(n) != 0) return -1;
    if (k->append) k->pos = n->len;
    size_t end = k->pos + size;
    if (end > n->cap) {
        size_t cap = n->cap ? n->cap : 64;
        while (cap < end) cap *= 2;
        n->data = (char *)realloc(n->data, cap);
        if (!n->data) abort();
        n->cap = cap;
    }
    if (k->pos > n->len) memset(n->data + n->len, 0, k->pos - n->len);
    memcpy(n->data + k->pos, buf, size);
    k->pos = end;
    if (end > n->len) n->len = end;
    n->mtime_ns = rask_sim_time_ns();
    return (ssize_t)size;
}

static int cookie_seek_to(Cookie *k, int64_t off, int whence, int64_t *out) {
    int64_t base = whence == SEEK_SET ? 0
                 : whence == SEEK_CUR ? (int64_t)k->pos
                 : node_size(k->node);
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
    Cookie *k = (Cookie *)c;
    if (k->real_fd >= 0) close(k->real_fd);
    node_release(k->node);
    free(k);
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

static FILE *open_stream(Node *n, const char *mode, int writes, int append) {
    Cookie *k = (Cookie *)xalloc(sizeof(Cookie));
    k->node = n;
    k->append = append;
    k->real_fd = -1;
    k->sick = node_sick(n);
    n->refs++;
#ifdef __APPLE__
    FILE *f = funopen(k, apple_read, writes ? apple_write : NULL, apple_seek, cookie_close);
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
    if (!f) {
        cookie_close(k);
        return NULL;
    }
    // One `fwrite`, one operation: a buffered stream would split a large
    // write across flushes, and a fault on the second would leave the first
    // half written (sim/F5). glibc then reports a failed write's full count
    // anyway and flags it only through `ferror`, which every writer checks.
    if (writes) setvbuf(f, NULL, _IONBF, 0);
    return f;
}

// ─── The calls runtime.c routes here ────────────────────────

FILE *rask_sim_fs_fopen(const char *path, const char *mode) {
    fs_step();
    char *np = canon(path);
    int plus = strchr(mode, '+') != NULL;
    int excl = strchr(mode, 'x') != NULL;
    Kind kind = kind_of(np);
    FILE *f = NULL;

    if (excl && kind != KIND_NONE) {
        errno = EEXIST;
    } else if (mode[0] == 'r') {
        Node *n = file_node(np, path);
        if (n) f = open_stream(n, mode, plus, 0);
    } else if (mode[0] == 'w' || mode[0] == 'a') {
        if (kind == KIND_DIR) {
            errno = EISDIR;
        } else if (kind == KIND_NONE && !parent_is_dir(np)) {
            errno = ENOENT;
        } else {
            Node *n = kind == KIND_NONE ? NULL : file_node(np, path);
            if (kind == KIND_NONE) {
                n = node_new(path, NULL);
                name_set(np, NAME_FILE, n);
            }
            if (n) {
                if (mode[0] == 'w') node_truncate(n);
                f = open_stream(n, mode, 1, mode[0] == 'a');
            }
        }
    } else {
        errno = EINVAL;
    }
    free(np);
    return f;
}

int rask_sim_fs_rename(const char *from, const char *to) {
    fs_step();
    char *nf = canon(from);
    char *nt = canon(to);
    Kind kind = kind_of(nf);
    int rc = -1;
    if (kind == KIND_NONE) {
        errno = ENOENT;
    } else if (kind == KIND_DIR) {
        rask_sim_unsimulated("`fs.rename(\"%s\", \"%s\")` on a directory", from, to);
    } else if (!parent_is_dir(nt)) {
        errno = ENOENT;
    } else if (kind_of(nt) == KIND_DIR) {
        errno = EISDIR;
    } else if (strcmp(nf, nt) == 0) {
        rc = 0;
    } else {
        Node *n = file_node(nf, from);
        if (n) {
            n->refs++;   // held across the two name changes
            name_set(nt, NAME_FILE, n);
            name_set(nf, NAME_GONE, NULL);
            node_release(n);
            rc = 0;
        }
    }
    free(nf);
    free(nt);
    return rc;
}

int rask_sim_fs_remove(const char *path) {
    fs_step();
    char *np = canon(path);
    Kind kind = kind_of(np);
    int rc = -1;
    if (kind == KIND_NONE) {
        errno = ENOENT;
    } else if (kind == KIND_DIR) {
        // Removing a directory needs to know it is empty, which means merging
        // the overlay with a real listing. Not modelled yet.
        rask_sim_unsimulated("`fs.remove(\"%s\")` on a directory", path);
    } else {
        name_set(np, NAME_GONE, NULL);
        rc = 0;
    }
    free(np);
    return rc;
}

int rask_sim_fs_mkdir(const char *path) {
    fs_step();
    char *np = canon(path);
    int rc = -1;
    if (kind_of(np) != KIND_NONE) {
        errno = EEXIST;
    } else if (!parent_is_dir(np)) {
        errno = ENOENT;
    } else {
        name_set(np, NAME_DIR, NULL);
        rc = 0;
    }
    free(np);
    return rc;
}

int rask_sim_fs_stat(const char *path, struct stat *st) {
    fs_step();
    char *np = canon(path);
    Name *e = find(np);
    int rc = 0;
    if (!np[0] || (e && e->kind == NAME_GONE) || (!e && ancestor_gone(np))) {
        errno = ENOENT;
        rc = -1;
    } else if (e && e->kind == NAME_FILE && e->node->backing) {
        rc = stat(e->node->backing, st);
    } else if (e) {
        memset(st, 0, sizeof(*st));
        st->st_mode = e->kind == NAME_DIR ? (S_IFDIR | 0755) : (S_IFREG | 0644);
        int64_t mtime = e->kind == NAME_FILE ? e->node->mtime_ns : 0;
        st->st_size = e->kind == NAME_FILE ? (off_t)e->node->len : 0;
        time_t secs = (time_t)(1577836800LL + mtime / 1000000000LL);
        st->st_mtime = secs;
        st->st_atime = secs;
    } else {
        rc = stat(np, st);
    }
    free(np);
    return rc;
}

static int name_cmp(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

// What the test would see in `path`: the real entries it hasn't touched plus
// the ones in the overlay, sorted. Sorted because readdir's order is the
// filesystem's, which differs between machines and so can't be part of a
// replay.
//
// NULL with errno set when there is no such directory.
char **rask_sim_fs_list(const char *path, size_t *count) {
    fs_step();
    char *np = canon(path);
    Kind kind = kind_of(np);
    if (kind != KIND_DIR) {
        errno = kind == KIND_NONE ? ENOENT : ENOTDIR;
        free(np);
        return NULL;
    }
    size_t n = 0, cap = 16;
    char **out = (char **)xalloc(cap * sizeof(char *));

#define PUSH(name) do { \
        if (n == cap) { \
            cap *= 2; \
            out = (char **)realloc(out, cap * sizeof(char *)); \
            if (!out) abort(); \
        } \
        out[n++] = xstrdup(name); \
    } while (0)

    // The real directory, unless the test made this one (then there is none).
    if (!find(np)) {
        DIR *d = opendir(np);
        if (d) {
            struct dirent *de;
            while ((de = readdir(d)) != NULL) {
                if (strcmp(de->d_name, ".") == 0 || strcmp(de->d_name, "..") == 0) continue;
                size_t len = strlen(np) + 1 + strlen(de->d_name) + 1;
                char *child = (char *)xalloc(len);
                snprintf(child, len, "%s/%s", strcmp(np, "/") == 0 ? "" : np, de->d_name);
                if (!find(child)) PUSH(de->d_name);   // an overlay name is listed below
                free(child);
            }
            closedir(d);
        }
    }
    for (Name *e = names; e; e = e->next) {
        if (e->kind == NAME_GONE || strcmp(e->path, "/") == 0) continue;
        char *parent = parent_of(e->path);
        if (strcmp(parent, np) == 0) PUSH(strrchr(e->path, '/') + 1);
        free(parent);
    }
#undef PUSH

    qsort(out, n, sizeof(char *), name_cmp);
    free(np);
    *count = n;
    return out;
}

#endif // RASK_SIM
