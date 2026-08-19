CC ?= cc
CFLAGS ?= -Wall -Wextra -Wpedantic -std=c11 -D_POSIX_C_SOURCE=200809L -O2
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
