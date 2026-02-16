CC ?= cc
CFLAGS ?= -Wall -Wextra -Wpedantic -std=c11 -D_POSIX_C_SOURCE=200809L -O2
AR ?= ar

SRCS = runtime.c alloc.c vec.c string.c map.c pool.c args.c \
       panic.c thread.c channel.c sync.c
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
