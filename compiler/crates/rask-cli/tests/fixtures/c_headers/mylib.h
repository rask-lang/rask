#ifndef MYLIB_H
#define MYLIB_H

#include <stdint.h>

#define MYLIB_VERSION 42
#define MYLIB_NAME "mylib"
#define MYLIB_MAX(a, b) ((a) > (b) ? (a) : (b))

typedef struct mylib_ctx mylib_ctx;

struct mylib_point {
    int x;
    int y;
};

typedef struct mylib_point mylib_point_t;

union mylib_value {
    int i;
    float f;
};

typedef enum {
    MYLIB_OK = 0,
    MYLIB_ERR = -1,
    MYLIB_TIMEOUT = -2,
} mylib_status;

int mylib_add(int a, int b);
void mylib_noop(void);
const char *mylib_version_string(void);
mylib_status mylib_init(mylib_ctx *ctx, const char *name);
uint32_t mylib_hash(const uint8_t *data, size_t len);

static int mylib_internal_helper(void) { return 0; }

extern int mylib_errno;

#endif
