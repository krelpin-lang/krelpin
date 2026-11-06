#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "krelpin.h"

extern char **environ;

/* The output of the last captured run on this thread, for `kr_process_captured`. */
static _Thread_local KrString *captured_out;
static _Thread_local KrString *captured_err;

/* A NUL-terminated copy of a string, or NULL if the string holds a NUL byte,
 * which can't be passed to a program. */
static char *c_string(const KrString *string) {
    if (memchr(string->data, 0, (size_t)string->len) != NULL) {
        kr_io_fail_nul();
        return NULL;
    }
    char *copy = kr_alloc(string->len + 1);
    memcpy(copy, string->data, (size_t)string->len);
    copy[string->len] = 0;
    return copy;
}

static void free_strings(char **strings) {
    if (strings == NULL) {
        return;
    }
    for (char **at = strings; *at != NULL; at++) {
        kr_free(*at);
    }
    kr_free(strings);
}

/* A NULL-terminated copy of an array of strings. */
static char **c_strings(const KrArray *array, int64_t extra) {
    KrString *const *items = (KrString *const *)array->data;
    char **strings = kr_alloc((array->len + extra + 1) * (int64_t)sizeof(char *));
    memset(strings, 0, (size_t)(array->len + extra + 1) * sizeof(char *));
    for (int64_t i = 0; i < array->len; i++) {
        strings[i] = c_string(items[i]);
        if (strings[i] == NULL) {
            free_strings(strings);
            return NULL;
        }
    }
    return strings;
}

static bool same_name(const char *a, const char *b) {
    size_t len = strcspn(a, "=");
    return strncmp(a, b, len) == 0 && b[len] == '=';
}

/* The environment of the child: this process's environment with the
 * `NAME=VALUE` overrides in `env` applied. */
static char **child_environment(const KrArray *env) {
    char **overrides = c_strings(env, 0);
    if (overrides == NULL) {
        return NULL;
    }
    int64_t inherited = 0;
    while (environ[inherited] != NULL) {
        inherited++;
    }
    char **result = kr_alloc((inherited + env->len + 1) * (int64_t)sizeof(char *));
    int64_t count = 0;
    for (int64_t i = 0; i < inherited; i++) {
        bool replaced = false;
        for (int64_t j = 0; j < env->len; j++) {
            if (same_name(overrides[j], environ[i])) {
                replaced = true;
                break;
            }
        }
        if (!replaced) {
            size_t len = strlen(environ[i]);
            result[count] = kr_alloc((int64_t)len + 1);
            memcpy(result[count], environ[i], len + 1);
            count++;
        }
    }
    for (int64_t j = 0; j < env->len; j++) {
        result[count++] = overrides[j];
    }
    result[count] = NULL;
    kr_free(overrides);
    return result;
}

typedef struct {
    char *data;
    int64_t len;
    int64_t cap;
} Buffer;

static void buffer_append(Buffer *buffer, const char *bytes, int64_t len) {
    if (buffer->len + len > buffer->cap) {
        int64_t cap = buffer->cap == 0 ? 4096 : buffer->cap;
        while (cap < buffer->len + len) {
            cap *= 2;
        }
        buffer->data = kr_realloc(buffer->data, cap);
        buffer->cap = cap;
    }
    memcpy(buffer->data + buffer->len, bytes, (size_t)len);
    buffer->len += len;
}

/* Reads both pipes until the child closes them, so that neither fills up
 * while the other is being waited on. */
static bool drain(int out_fd, int err_fd, Buffer *out, Buffer *err) {
    struct pollfd fds[2] = {{out_fd, POLLIN, 0}, {err_fd, POLLIN, 0}};
    Buffer *buffers[2] = {out, err};
    int open_count = 2;
    char chunk[8192];
    while (open_count > 0) {
        if (poll(fds, 2, -1) < 0) {
            if (errno == EINTR) {
                continue;
            }
            return kr_io_fail_errno();
        }
        for (int i = 0; i < 2; i++) {
            if (fds[i].fd < 0 || fds[i].revents == 0) {
                continue;
            }
            ssize_t got = read(fds[i].fd, chunk, sizeof chunk);
            if (got < 0 && errno == EINTR) {
                continue;
            }
            if (got <= 0) {
                close(fds[i].fd);
                fds[i].fd = -1;
                open_count--;
                continue;
            }
            buffer_append(buffers[i], chunk, (int64_t)got);
        }
    }
    return true;
}

static KrString *take_text(Buffer *buffer) {
    KrString *text = NULL;
    if (kr_utf8_valid(buffer->data, buffer->len)) {
        text = kr_string_new(buffer->data == NULL ? "" : buffer->data, buffer->len);
    }
    kr_free(buffer->data);
    return text;
}

static void close_pipe(int fds[2]) {
    if (fds[0] >= 0) {
        close(fds[0]);
    }
    if (fds[1] >= 0) {
        close(fds[1]);
    }
}

static bool open_pipe(int fds[2]) {
    if (pipe(fds) != 0) {
        fds[0] = fds[1] = -1;
        return kr_io_fail_errno();
    }
    fcntl(fds[0], F_SETFD, FD_CLOEXEC);
    fcntl(fds[1], F_SETFD, FD_CLOEXEC);
    return true;
}

/* Runs `argv[0]` with the arguments `argv[1..]`, found on the PATH, in
 * `directory` (or the current one) with `env` added to the environment, and
 * waits for it. With `capture`, its output is kept for `kr_process_captured`;
 * otherwise it shares this process's standard streams.
 *
 * Returns the exit code, 128 + the signal number if a signal killed it, or
 * -1 with the error recorded if it couldn't be started. */
int32_t kr_process_run(const KrArray *argv, const KrString *directory, const KrArray *env,
                       bool capture) {
    kr_io_clear_error();
    if (argv->len == 0) {
        kr_io_fail(EINVAL);
        return -1;
    }
    char **args = c_strings(argv, 0);
    char *dir = directory == NULL ? NULL : c_string(directory);
    char **envp = child_environment(env);
    int out_pipe[2] = {-1, -1};
    int err_pipe[2] = {-1, -1};
    int exec_pipe[2] = {-1, -1};
    int32_t status = -1;
    if (args == NULL || (directory != NULL && dir == NULL) || envp == NULL) {
        goto done;
    }
    if ((capture && (!open_pipe(out_pipe) || !open_pipe(err_pipe))) || !open_pipe(exec_pipe)) {
        goto done;
    }
    /* Output written so far must come before the child's. */
    fflush(NULL);
    pid_t pid = fork();
    if (pid < 0) {
        kr_io_fail_errno();
        goto done;
    }
    if (pid == 0) {
        /* Only async-signal-safe calls from here on. */
        if (capture) {
            dup2(out_pipe[1], STDOUT_FILENO);
            dup2(err_pipe[1], STDERR_FILENO);
        }
        int error = 0;
        if (dir != NULL && chdir(dir) != 0) {
            error = errno;
        } else {
            environ = envp;
            execvp(args[0], args);
            error = errno;
        }
        ssize_t ignored = write(exec_pipe[1], &error, sizeof error);
        (void)ignored;
        _exit(127);
    }
    close(exec_pipe[1]);
    exec_pipe[1] = -1;
    if (capture) {
        close(out_pipe[1]);
        close(err_pipe[1]);
        out_pipe[1] = err_pipe[1] = -1;
    }
    /* The pipe closes without data when `execvp` succeeds. */
    int exec_error = 0;
    ssize_t got;
    do {
        got = read(exec_pipe[0], &exec_error, sizeof exec_error);
    } while (got < 0 && errno == EINTR);
    Buffer out = {0};
    Buffer err = {0};
    bool drained = true;
    if (capture) {
        drained = drain(out_pipe[0], err_pipe[0], &out, &err);
        out_pipe[0] = err_pipe[0] = -1;
    }
    int wait_status = 0;
    while (waitpid(pid, &wait_status, 0) < 0) {
        if (errno != EINTR) {
            kr_io_fail_errno();
            kr_free(out.data);
            kr_free(err.data);
            goto done;
        }
    }
    if (got == (ssize_t)sizeof exec_error) {
        kr_io_fail(exec_error);
        kr_free(out.data);
        kr_free(err.data);
        goto done;
    }
    if (!drained) {
        kr_free(out.data);
        kr_free(err.data);
        goto done;
    }
    if (capture) {
        kr_string_release(captured_out);
        kr_string_release(captured_err);
        captured_out = take_text(&out);
        captured_err = take_text(&err);
        if (captured_out == NULL || captured_err == NULL) {
            kr_io_fail_not_utf8();
            goto done;
        }
    }
    if (WIFEXITED(wait_status)) {
        status = WEXITSTATUS(wait_status);
    } else if (WIFSIGNALED(wait_status)) {
        status = 128 + WTERMSIG(wait_status);
    } else {
        status = 128;
    }
done:
    close_pipe(out_pipe);
    close_pipe(err_pipe);
    close_pipe(exec_pipe);
    free_strings(args);
    kr_free(dir);
    free_strings(envp);
    return status;
}

/* Hands over the standard output (or with `error`, the standard error) of
 * the last captured run on this thread. */
KrString *kr_process_captured(bool error) {
    KrString **slot = error ? &captured_err : &captured_out;
    KrString *text = *slot;
    *slot = NULL;
    return text == NULL ? kr_string_new("", 0) : text;
}
