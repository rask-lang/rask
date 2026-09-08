// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Subprocess execution — `os.Command.run()` (std.os).
//
// The builder lives in Rask (`stdlib/os.rk`); this is only the part that needs
// the OS. `rask_process_run` forks, execs, drains the pipes and waits; the two
// readers hand back what it captured. Splitting the answer across three calls
// avoids a struct crossing the FFI boundary, whose layout C would have to know
// and keep in step with the compiler's.
//
// The captured output is thread-local and belongs to the most recent run on
// that thread. `Command.run` reads it on the next two lines, so there is no
// window for another run to overwrite it.

#include "rask_runtime.h"

#include <errno.h>
#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#define RASK_NO_SUBPROCESS 1
#else
#include <fcntl.h>
#include <signal.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

// std.os: how a child's stream is wired. Matches `Stdio` in stdlib/os.rk.
#define RASK_STDIO_INHERIT 0
#define RASK_STDIO_PIPED   1
#define RASK_STDIO_NULL    2

typedef struct {
    char  *data;
    size_t len;
} Captured;

static _Thread_local Captured g_out = {NULL, 0};
static _Thread_local Captured g_err = {NULL, 0};

static void captured_reset(Captured *c) {
    free(c->data);
    c->data = NULL;
    c->len = 0;
}

// A NUL-terminated copy of a length-counted Rask string. execvp and chdir both
// want C strings, and a RaskStr's bytes are not guaranteed terminated.
static char *cstr_of(const RaskStr *s) {
    int64_t len = s ? rask_string_len(s) : 0;
    char *out = (char *)malloc((size_t)len + 1);
    if (!out) return NULL;
    if (len > 0) memcpy(out, rask_string_ptr(s), (size_t)len);
    out[len] = '\0';
    return out;
}

#ifndef RASK_NO_SUBPROCESS

// Read a pipe to EOF into `c`. The child can outproduce any fixed buffer, so
// this grows; a child that writes forever will fill memory, which is the same
// bargain every capturing runner makes.
static int drain(int fd, Captured *c) {
    size_t cap = 4096;
    c->data = (char *)malloc(cap);
    if (!c->data) return -1;
    c->len = 0;
    for (;;) {
        if (c->len + 1024 > cap) {
            size_t next = cap * 2;
            char *bigger = (char *)realloc(c->data, next);
            if (!bigger) return -1;
            c->data = bigger;
            cap = next;
        }
        ssize_t n = read(fd, c->data + c->len, cap - c->len);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) break;
        c->len += (size_t)n;
    }
    return 0;
}

// Point `fd` at /dev/null, for a stream configured Null (or a stdin the child
// must not be able to read from).
static void redirect_to_null(int fd) {
    int devnull = open("/dev/null", O_RDWR);
    if (devnull >= 0) {
        dup2(devnull, fd);
        if (devnull != fd) close(devnull);
    }
}

#endif // !RASK_NO_SUBPROCESS

// Run `program` with `args`, `envs` (flattened key, value, key, value…) and an
// optional working directory.
//
// Returns the child's exit status, or a negative errno if it never started —
// `Command.run` turns -ENOENT into a NotFound and -EACCES into a
// PermissionDenied. The output captured for a Piped stream is readable through
// the two functions below until the next run on this thread.
int64_t rask_process_run(
    const RaskStr *program,
    const RaskVec *args,
    const RaskVec *envs,
    const RaskStr *dir,
    int64_t stdin_mode,
    int64_t stdout_mode,
    int64_t stderr_mode
) {
    captured_reset(&g_out);
    captured_reset(&g_err);

#ifdef RASK_NO_SUBPROCESS
    (void)program; (void)args; (void)envs; (void)dir;
    (void)stdin_mode; (void)stdout_mode; (void)stderr_mode;
    return -1;
#else
    int64_t argc = args ? rask_vec_len(args) : 0;
    char **argv = (char **)calloc((size_t)argc + 2, sizeof(char *));
    if (!argv) return -1;
    argv[0] = cstr_of(program);
    for (int64_t i = 0; i < argc; i++) {
        argv[i + 1] = cstr_of((const RaskStr *)rask_vec_get(args, i));
    }

    char *cwd = NULL;
    if (dir && rask_string_len(dir) > 0) cwd = cstr_of(dir);

    // Flattened pairs, so an odd length means the last key has no value.
    int64_t env_n = envs ? rask_vec_len(envs) / 2 : 0;

    int out_pipe[2] = {-1, -1};
    int err_pipe[2] = {-1, -1};
    // How the child tells us exec failed. It closes on a successful exec, so
    // the parent reading nothing means the program started; anything read is
    // the child's errno. Without it a missing program is indistinguishable
    // from one that ran and exited 127.
    int exec_pipe[2] = {-1, -1};
    int failed = 0;
    if (stdout_mode == RASK_STDIO_PIPED && pipe(out_pipe) != 0) failed = 1;
    if (!failed && stderr_mode == RASK_STDIO_PIPED && pipe(err_pipe) != 0) failed = 1;
    if (!failed && pipe(exec_pipe) != 0) failed = 1;
    if (!failed) fcntl(exec_pipe[1], F_SETFD, FD_CLOEXEC);

    pid_t pid = failed ? -1 : fork();
    if (pid == 0) {
        // Child. Everything after this either execs or _exits — a failure here
        // must not run the parent's cleanup or its atexit handlers.
        if (out_pipe[1] >= 0) {
            dup2(out_pipe[1], 1);
            close(out_pipe[0]);
            close(out_pipe[1]);
        } else if (stdout_mode == RASK_STDIO_NULL) {
            redirect_to_null(1);
        }
        if (err_pipe[1] >= 0) {
            dup2(err_pipe[1], 2);
            close(err_pipe[0]);
            close(err_pipe[1]);
        } else if (stderr_mode == RASK_STDIO_NULL) {
            redirect_to_null(2);
        }
        // `run` waits for the child, so there is nothing to write to its stdin
        // — a Piped stdin here would be a pipe nobody feeds, and the child
        // would block on a read forever.
        if (stdin_mode != RASK_STDIO_INHERIT) {
            redirect_to_null(0);
        }
        close(exec_pipe[0]);
        if (cwd && chdir(cwd) != 0) {
            int e = errno;
            ssize_t ignored = write(exec_pipe[1], &e, sizeof(e));
            (void)ignored;
            _exit(127);
        }
        for (int64_t i = 0; i < env_n; i++) {
            char *k = cstr_of((const RaskStr *)rask_vec_get(envs, i * 2));
            char *v = cstr_of((const RaskStr *)rask_vec_get(envs, i * 2 + 1));
            if (k && v) setenv(k, v, 1);
            free(k);
            free(v);
        }
        execvp(argv[0], argv);
        // Only reached when exec failed. Tell the parent why, then leave with
        // the shell's "command not found" status in case it isn't listening.
        int e = errno;
        ssize_t ignored = write(exec_pipe[1], &e, sizeof(e));
        (void)ignored;
        _exit(127);
    }

    int64_t status = -1;
    if (pid > 0) {
        if (out_pipe[1] >= 0) close(out_pipe[1]);
        if (err_pipe[1] >= 0) close(err_pipe[1]);
        close(exec_pipe[1]);
        int child_errno = 0;
        ssize_t got = read(exec_pipe[0], &child_errno, sizeof(child_errno));
        close(exec_pipe[0]);
        // Drain both before waiting: a child that fills a pipe buffer blocks
        // until someone reads it, and waiting first would deadlock there.
        if (out_pipe[0] >= 0) {
            drain(out_pipe[0], &g_out);
            close(out_pipe[0]);
        }
        if (err_pipe[0] >= 0) {
            drain(err_pipe[0], &g_err);
            close(err_pipe[0]);
        }
        int wstatus = 0;
        while (waitpid(pid, &wstatus, 0) < 0 && errno == EINTR) {
        }
        if (got == (ssize_t)sizeof(child_errno) && child_errno != 0) {
            // Never started. The negative errno is what `Command.run` turns
            // into a NotFound or PermissionDenied.
            status = -(int64_t)child_errno;
        } else if (WIFEXITED(wstatus)) {
            status = (int64_t)WEXITSTATUS(wstatus);
        } else if (WIFSIGNALED(wstatus)) {
            // What a shell reports for a signalled child.
            status = 128 + (int64_t)WTERMSIG(wstatus);
        }
    } else {
        if (out_pipe[0] >= 0) close(out_pipe[0]);
        if (out_pipe[1] >= 0) close(out_pipe[1]);
        if (err_pipe[0] >= 0) close(err_pipe[0]);
        if (err_pipe[1] >= 0) close(err_pipe[1]);
        if (exec_pipe[0] >= 0) close(exec_pipe[0]);
        if (exec_pipe[1] >= 0) close(exec_pipe[1]);
    }

    for (int64_t i = 0; i <= argc; i++) free(argv[i]);
    free(argv);
    free(cwd);
    return status;
#endif
}

// The stdout captured by the last `rask_process_run` on this thread. Empty when
// the stream wasn't piped or nothing was written.
void rask_process_stdout(RaskStr *out) {
    rask_string_from_bytes(out, g_out.data ? g_out.data : "", (int64_t)g_out.len);
}

void rask_process_stderr(RaskStr *out) {
    rask_string_from_bytes(out, g_err.data ? g_err.data : "", (int64_t)g_err.len);
}

// ─── Spawned processes (std.os/C3, C4) ──────────────────────
//
// `run` forks, drains and waits in one call. `spawn` hands the handle back
// instead, and the six methods on `Process` work through it. The handle is the
// address of one of these, and `Process` in `stdlib/os.rk` carries it as an
// `i64` — the same trick every other opaque runtime object uses, so no struct
// layout has to cross the boundary.
//
// A piped stream stays open in the parent: stdout so `read_stdout` has
// something to read, stdin so `write_stdin` has somewhere to write. `wait`
// closes stdin before it waits — a child reading to EOF would otherwise never
// exit, and "I am done writing" is exactly what waiting means.

typedef struct {
    int      pid;
    int      in_fd;
    int      out_fd;
    int      err_fd;
    int      reaped;
    int64_t  status;
    Captured out;
    Captured err;
} RaskProcess;

static RaskProcess *proc_of(int64_t handle) {
    return (RaskProcess *)(intptr_t)handle;
}

#ifndef RASK_NO_SUBPROCESS

// Everything `run` does up to the fork, shared with `spawn`: the argv, the
// working directory, the pipes. Returns 0 on success with the pieces filled in.
static void proc_close(int *fd) {
    if (*fd >= 0) {
        close(*fd);
        *fd = -1;
    }
}

// Read whatever is left on `fd` and append it to `c`, then close it.
static void drain_into(int *fd, Captured *c) {
    if (*fd < 0) return;
    Captured more = {NULL, 0};
    if (drain(*fd, &more) == 0 && more.len > 0) {
        char *joined = (char *)realloc(c->data, c->len + more.len);
        if (joined) {
            memcpy(joined + c->len, more.data, more.len);
            c->data = joined;
            c->len += more.len;
        }
    }
    free(more.data);
    proc_close(fd);
}

static int64_t proc_reap(RaskProcess *p, int blocking) {
    if (p->reaped) return p->status;
    int wstatus = 0;
    int flags = blocking ? 0 : WNOHANG;
    pid_t got;
    while ((got = waitpid(p->pid, &wstatus, flags)) < 0 && errno == EINTR) {
    }
    if (got == 0) return -1;              // still running (non-blocking only)
    if (got < 0) {
        p->reaped = 1;
        p->status = -(int64_t)errno;
        return p->status;
    }
    p->reaped = 1;
    if (WIFEXITED(wstatus)) {
        p->status = (int64_t)WEXITSTATUS(wstatus);
    } else if (WIFSIGNALED(wstatus)) {
        p->status = 128 + (int64_t)WTERMSIG(wstatus);
    } else {
        p->status = -1;
    }
    return p->status;
}

#endif // !RASK_NO_SUBPROCESS

// Start `program` and hand back a handle, or a negative errno if it never
// started. The child's errno arrives through the same close-on-exec pipe `run`
// uses, so "no such program" is still distinguishable from "exited 127".
int64_t rask_process_spawn(
    const RaskStr *program,
    const RaskVec *args,
    const RaskVec *envs,
    const RaskStr *dir,
    int64_t stdin_mode,
    int64_t stdout_mode,
    int64_t stderr_mode
) {
#ifdef RASK_NO_SUBPROCESS
    (void)program; (void)args; (void)envs; (void)dir;
    (void)stdin_mode; (void)stdout_mode; (void)stderr_mode;
    return -1;
#else
    int64_t argc = args ? rask_vec_len(args) : 0;
    char **argv = (char **)calloc((size_t)argc + 2, sizeof(char *));
    if (!argv) return -(int64_t)ENOMEM;
    argv[0] = cstr_of(program);
    for (int64_t i = 0; i < argc; i++) {
        argv[i + 1] = cstr_of((const RaskStr *)rask_vec_get(args, i));
    }
    char *cwd = NULL;
    if (dir && rask_string_len(dir) > 0) cwd = cstr_of(dir);
    int64_t env_n = envs ? rask_vec_len(envs) / 2 : 0;

    int in_pipe[2] = {-1, -1};
    int out_pipe[2] = {-1, -1};
    int err_pipe[2] = {-1, -1};
    int exec_pipe[2] = {-1, -1};
    int failed = 0;
    if (stdin_mode == RASK_STDIO_PIPED && pipe(in_pipe) != 0) failed = 1;
    if (!failed && stdout_mode == RASK_STDIO_PIPED && pipe(out_pipe) != 0) failed = 1;
    if (!failed && stderr_mode == RASK_STDIO_PIPED && pipe(err_pipe) != 0) failed = 1;
    if (!failed && pipe(exec_pipe) != 0) failed = 1;
    if (!failed) fcntl(exec_pipe[1], F_SETFD, FD_CLOEXEC);

    pid_t pid = failed ? -1 : fork();
    if (pid == 0) {
        if (in_pipe[0] >= 0) {
            dup2(in_pipe[0], 0);
            close(in_pipe[0]);
            close(in_pipe[1]);
        } else if (stdin_mode == RASK_STDIO_NULL) {
            redirect_to_null(0);
        }
        if (out_pipe[1] >= 0) {
            dup2(out_pipe[1], 1);
            close(out_pipe[0]);
            close(out_pipe[1]);
        } else if (stdout_mode == RASK_STDIO_NULL) {
            redirect_to_null(1);
        }
        if (err_pipe[1] >= 0) {
            dup2(err_pipe[1], 2);
            close(err_pipe[0]);
            close(err_pipe[1]);
        } else if (stderr_mode == RASK_STDIO_NULL) {
            redirect_to_null(2);
        }
        close(exec_pipe[0]);
        if (cwd && chdir(cwd) != 0) {
            int e = errno;
            ssize_t ignored = write(exec_pipe[1], &e, sizeof(e));
            (void)ignored;
            _exit(127);
        }
        for (int64_t i = 0; i < env_n; i++) {
            char *k = cstr_of((const RaskStr *)rask_vec_get(envs, i * 2));
            char *v = cstr_of((const RaskStr *)rask_vec_get(envs, i * 2 + 1));
            if (k && v) setenv(k, v, 1);
            free(k);
            free(v);
        }
        execvp(argv[0], argv);
        int e = errno;
        ssize_t ignored = write(exec_pipe[1], &e, sizeof(e));
        (void)ignored;
        _exit(127);
    }

    int64_t answer;
    if (pid > 0) {
        proc_close(&in_pipe[0]);
        proc_close(&out_pipe[1]);
        proc_close(&err_pipe[1]);
        proc_close(&exec_pipe[1]);
        int child_errno = 0;
        ssize_t got = read(exec_pipe[0], &child_errno, sizeof(child_errno));
        proc_close(&exec_pipe[0]);
        if (got == (ssize_t)sizeof(child_errno) && child_errno != 0) {
            // Never started. Reap the shell-status child and report why.
            int wstatus = 0;
            while (waitpid(pid, &wstatus, 0) < 0 && errno == EINTR) {
            }
            proc_close(&in_pipe[1]);
            proc_close(&out_pipe[0]);
            proc_close(&err_pipe[0]);
            answer = -(int64_t)child_errno;
        } else {
            RaskProcess *p = (RaskProcess *)rask_alloc((int64_t)sizeof(RaskProcess));
            memset(p, 0, sizeof(*p));
            p->pid = (int)pid;
            p->in_fd = in_pipe[1];
            p->out_fd = out_pipe[0];
            p->err_fd = err_pipe[0];
            p->status = -1;
            answer = (int64_t)(intptr_t)p;
        }
    } else {
        for (int i = 0; i < 2; i++) {
            proc_close(&in_pipe[i]);
            proc_close(&out_pipe[i]);
            proc_close(&err_pipe[i]);
            proc_close(&exec_pipe[i]);
        }
        answer = -(int64_t)errno;
    }

    for (int64_t i = 0; i <= argc; i++) free(argv[i]);
    free(argv);
    free(cwd);
    return answer;
#endif
}

int64_t rask_process_pid(int64_t handle) {
    RaskProcess *p = proc_of(handle);
    return p ? (int64_t)p->pid : -1;
}

// Blocking wait. Closes stdin first — a child reading to EOF would never exit
// otherwise — then drains what is left of both pipes so `Output` carries it.
int64_t rask_process_wait(int64_t handle) {
    RaskProcess *p = proc_of(handle);
    if (!p) return -1;
#ifdef RASK_NO_SUBPROCESS
    return -1;
#else
    proc_close(&p->in_fd);
    drain_into(&p->out_fd, &p->out);
    drain_into(&p->err_fd, &p->err);
    return proc_reap(p, 1);
#endif
}

int64_t rask_process_kill_and_wait(int64_t handle) {
    RaskProcess *p = proc_of(handle);
    if (!p) return -1;
#ifdef RASK_NO_SUBPROCESS
    return -1;
#else
    if (!p->reaped) kill(p->pid, SIGKILL);
    return rask_process_wait(handle);
#endif
}

// -1 while the child is still running; its status once it has exited.
int64_t rask_process_poll(int64_t handle) {
    RaskProcess *p = proc_of(handle);
    if (!p) return -1;
#ifdef RASK_NO_SUBPROCESS
    return -1;
#else
    int64_t status = proc_reap(p, 0);
    if (p->reaped) {
        drain_into(&p->out_fd, &p->out);
        drain_into(&p->err_fd, &p->err);
    }
    return status;
#endif
}

// 0, or a negative errno. Writing to a child that has closed its stdin gives
// EPIPE rather than the SIGPIPE that would kill the parent.
int64_t rask_process_write_stdin(int64_t handle, const RaskStr *data) {
    RaskProcess *p = proc_of(handle);
    if (!p || p->in_fd < 0) return -(int64_t)EBADF;
#ifdef RASK_NO_SUBPROCESS
    (void)data;
    return -1;
#else
    int64_t len = data ? rask_string_len(data) : 0;
    const char *bytes = data ? rask_string_ptr(data) : "";
    int64_t written = 0;
    while (written < len) {
        ssize_t n = write(p->in_fd, bytes + written, (size_t)(len - written));
        if (n < 0) {
            if (errno == EINTR) continue;
            return -(int64_t)errno;
        }
        written += n;
    }
    return 0;
#endif
}

// Everything the child has written to stdout, up to EOF. Blocking, and the
// same on both backends — "what is available right now" would be a different
// answer per run and per platform.
void rask_process_read_stdout(RaskStr *out, int64_t handle) {
    RaskProcess *p = proc_of(handle);
#ifndef RASK_NO_SUBPROCESS
    if (p) drain_into(&p->out_fd, &p->out);
#endif
    if (!p) {
        rask_string_from_bytes(out, "", 0);
        return;
    }
    rask_string_from_bytes(out, p->out.data ? p->out.data : "", (int64_t)p->out.len);
    captured_reset(&p->out);
}

// What `wait` collected, for `Output`. Separate calls for the same reason
// `run`'s two readers are separate: no struct crosses the boundary.
void rask_process_captured_stdout(RaskStr *out, int64_t handle) {
    RaskProcess *p = proc_of(handle);
    rask_string_from_bytes(out, (p && p->out.data) ? p->out.data : "",
                           p ? (int64_t)p->out.len : 0);
}

void rask_process_captured_stderr(RaskStr *out, int64_t handle) {
    RaskProcess *p = proc_of(handle);
    rask_string_from_bytes(out, (p && p->err.data) ? p->err.data : "",
                           p ? (int64_t)p->err.len : 0);
}

// Release the handle. The child is already reaped by then — `Process` is
// `@resource`, so it cannot be dropped without `wait` or `kill_and_wait`
// (std.os/C4).
void rask_process_release(int64_t handle) {
    RaskProcess *p = proc_of(handle);
    if (!p) return;
#ifndef RASK_NO_SUBPROCESS
    proc_close(&p->in_fd);
    proc_close(&p->out_fd);
    proc_close(&p->err_fd);
#endif
    captured_reset(&p->out);
    captured_reset(&p->err);
    rask_realloc(p, (int64_t)sizeof(RaskProcess), 0);
}
