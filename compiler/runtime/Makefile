CC ?= cc
# POSIX 2008 plus each platform's extensions. The runtime uses two things POSIX
# doesn't have — `dladdr` for symbol names in leak traces, `_SC_NPROCESSORS_ONLN`
# for the default worker count — and asking for strict POSIX hides them. glibc
# exposes them anyway, so Linux never noticed; Darwin actually enforces it, and
# the whole v0.2.0 release stalled on this twice. `_GNU_SOURCE` is inert on
# Darwin and `_DARWIN_C_SOURCE` is inert on glibc, so both go to both.
# `feature_cflags()` in rask-cli/src/commands/link.rs passes the same pair; that
# is the compile every Rask program actually gets, and the two must not drift.
CFLAGS ?= -Wall -Wextra -Wpedantic -std=c11 -D_POSIX_C_SOURCE=200809L -D_GNU_SOURCE -D_DARWIN_C_SOURCE -O2
AR ?= ar

# Every C file in this directory. A hand-written list went stale — it was
# missing random.c, time.c, test.c, bench.c, atomic.c, ptr.c, simd.c and the two
# io engines, so `make` reported "Nothing to be done" after editing one of them
# and the archive silently lacked it. The list that actually builds a program is
# in rask-cli/src/commands/link.rs, which splits by platform; this one is the
# developer convenience and has no reason to be selective.
SRCS = $(wildcard *.c)
OBJS = $(SRCS:.c=.o)
LIB  = librask_runtime.a

.PHONY: all clean

all: $(LIB)

$(LIB): $(OBJS)
	$(AR) rcs $@ $^

%.o: %.c rask_runtime.h
	$(CC) $(CFLAGS) -c $< -o $@

clean:
	rm -f $(OBJS) $(LIB)
