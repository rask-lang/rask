// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline for examples/grep_clone.rk. Same flags, same output, same exit
// codes. Written to be idiomatic and correctly error-handled, not golfed —
// see README.md for the counting rules this is measured under.

#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    const char *pattern;
    const char **files;
    size_t file_count;
    int ignore_case;
    int line_numbers;
    int count_only;
    int invert_match;
} GrepOptions;

static void print_usage(void) {
    puts("Usage: rgrep [OPTIONS] PATTERN FILE...");
    puts("");
    puts("Options:");
    puts("  -i    Ignore case");
    puts("  -n    Show line numbers");
    puts("  -c    Count matching lines only");
    puts("  -v    Invert match (show non-matching lines)");
    puts("  -h    Show this help");
}

static int line_matches(const char *line, const char *pattern, int ignore_case) {
    if (ignore_case) {
        return strcasestr(line, pattern) != NULL;
    }
    return strstr(line, pattern) != NULL;
}

// Match count, or -1 on error (message already printed).
static long grep_file(const char *path, const GrepOptions *opts) {
    FILE *f = fopen(path, "r");
    if (f == NULL) {
        printf("rgrep: %s: %s\n", path, strerror(errno));
        return -1;
    }

    char *line = NULL;
    size_t cap = 0;
    long match_count = 0;
    long line_num = 0;
    ssize_t len;

    while ((len = getline(&line, &cap, f)) != -1) {
        line_num++;
        if (len > 0 && line[len - 1] == '\n') {
            line[len - 1] = '\0';
        }

        int matches = line_matches(line, opts->pattern, opts->ignore_case);
        int show = opts->invert_match ? !matches : matches;

        if (show) {
            match_count++;
            if (!opts->count_only) {
                if (opts->line_numbers) {
                    printf("%ld:%s\n", line_num, line);
                } else {
                    printf("%s\n", line);
                }
            }
        }
    }

    if (ferror(f)) {
        printf("rgrep: %s: read error\n", path);
        free(line);
        fclose(f);
        return -1;
    }

    free(line);
    fclose(f);

    if (opts->count_only) {
        printf("%ld\n", match_count);
    }
    return match_count;
}

int main(int argc, char **argv) {
    GrepOptions opts = {0};

    const char **positional = malloc((size_t)argc * sizeof(*positional));
    if (positional == NULL) {
        puts("rgrep: out of memory");
        return 2;
    }
    size_t pos_count = 0;

    for (int i = 1; i < argc; i++) {
        const char *arg = argv[i];
        if (strcmp(arg, "-i") == 0) {
            opts.ignore_case = 1;
        } else if (strcmp(arg, "-n") == 0) {
            opts.line_numbers = 1;
        } else if (strcmp(arg, "-c") == 0) {
            opts.count_only = 1;
        } else if (strcmp(arg, "-v") == 0) {
            opts.invert_match = 1;
        } else if (strcmp(arg, "-h") == 0 || strcmp(arg, "--help") == 0) {
            print_usage();
            free(positional);
            return 0;
        } else if (strcmp(arg, "--") == 0) {
            // skip
        } else {
            positional[pos_count++] = arg;
        }
    }

    if (pos_count == 0) {
        puts("rgrep: missing pattern argument");
        print_usage();
        free(positional);
        return 1;
    }
    if (pos_count < 2) {
        puts("rgrep: no files specified");
        print_usage();
        free(positional);
        return 1;
    }

    opts.pattern = positional[0];
    opts.files = positional + 1;
    opts.file_count = pos_count - 1;

    long total_matches = 0;
    int had_errors = 0;

    for (size_t i = 0; i < opts.file_count; i++) {
        long found = grep_file(opts.files[i], &opts);
        if (found < 0) {
            had_errors = 1;
        } else {
            total_matches += found;
        }
    }

    free(positional);

    if (had_errors) {
        return 2;
    }
    if (total_matches == 0) {
        return 1;
    }
    return 0;
}
