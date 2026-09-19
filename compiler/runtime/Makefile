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

# ...except the three that only exist on Linux: epoll, io_uring, and a green
# scheduler that has no kqueue backend yet. The wildcard can't know that, so on
# Darwin `make` reached io_epoll_engine.c and died on a missing <sys/epoll.h> —
# it was building a set no macOS program has ever linked. `LINUX_SOURCES` in
# rask-cli/src/commands/link.rs is the same three names for the compile that
# ships; that list is deliberately written down ("knowledge, not something a
# listing can tell you") and this is the second copy of it. What keeps the two
# honest is the macos-runtime job in .github/workflows/test.yml: adding a
# Linux-only source without adding it here turns that job red on the same PR.
LINUX_ONLY = green.c io_uring_engine.c io_epoll_engine.c
ifneq ($(shell uname -s),Linux)
SRCS := $(filter-out $(LINUX_ONLY),$(SRCS))
endif

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
