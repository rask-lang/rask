// SPDX-License-Identifier: (MIT OR Apache-2.0)

// CLI args — stores argc/argv from main() for access by Rask programs.
// Environment variable access lives here too: same "what the process was
// started with" surface.

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

static int    g_argc = 0;
static char **g_argv = NULL;

void rask_args_init(int argc, char **argv) {
    g_argc = argc;
    g_argv = argv;
}

int64_t rask_args_count(void) {
    return (int64_t)g_argc;
}

const char *rask_args_get(int64_t index) {
    if (index < 0 || index >= g_argc) return NULL;
    return g_argv[index];
}

// ─── Environment variables ──────────────────────────────────

// getenv() needs a NUL-terminated name; a RaskStr is length-counted and its
// heap form isn't guaranteed terminated, so copy into a bounded buffer.
// Names longer than this can't be real environment variables.
#define RASK_ENV_NAME_MAX 512

static const char *env_lookup(const RaskStr *name) {
    if (!name) return NULL;
    int64_t len = rask_string_len(name);
    if (len <= 0 || len >= RASK_ENV_NAME_MAX) return NULL;
    char buf[RASK_ENV_NAME_MAX];
    memcpy(buf, rask_string_ptr(name), (size_t)len);
    buf[len] = '\0';
    return getenv(buf);
}

// os.env(name) -> string? — NULL when unset, which codegen turns into `none`.
// The RaskStr is heap-allocated because the caller copies 16 bytes out of it.
const RaskStr *rask_os_env(const RaskStr *name) {
    const char *val = env_lookup(name);
    if (!val) return NULL;
    RaskStr *out = (RaskStr *)rask_alloc((int64_t)sizeof(RaskStr));
    rask_string_from(out, val);
    return out;
}

// os.pid() -> i64
int64_t rask_os_pid(void) {
    return (int64_t)getpid();
}

// os.env_vars() -> Vec<(string, string)>
//
// Pairs laid out the way a tuple is: name at 0, value at 16. `environ` gives
// them as "NAME=VALUE"; the first '=' splits them.
RaskVec *rask_os_env_vars(void) {
    RaskVec *v = rask_vec_new(32);
    if (!environ) return v;
    char pair[32];
    for (char **e = environ; *e; e++) {
        const char *eq = strchr(*e, '=');
        if (!eq) continue;
        memset(pair, 0, sizeof(pair));
        rask_string_from_bytes((RaskStr *)pair, *e, (int64_t)(eq - *e));
        rask_string_from((RaskStr *)(pair + 16), eq + 1);
        rask_vec_push(v, pair);
    }
    return v;
}

// os.env_or(name, default) -> string
void rask_os_env_or(RaskStr *out, const RaskStr *name, const RaskStr *def) {
    const char *val = env_lookup(name);
    if (val) {
        rask_string_from(out, val);
        return;
    }
    if (def) {
        rask_string_from_bytes(out, rask_string_ptr(def), rask_string_len(def));
    } else {
        rask_string_new(out);
    }
}
