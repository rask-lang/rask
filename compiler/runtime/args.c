// SPDX-License-Identifier: (MIT OR Apache-2.0)

// CLI args — stores argc/argv from main() for access by Rask programs.

#include "rask_runtime.h"

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
