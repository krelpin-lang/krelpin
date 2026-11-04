#define _GNU_SOURCE

#include <locale.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <wctype.h>

#include "krelpin.h"

static int saved_argc;
static char **saved_argv;
static pthread_mutex_t console_lock = PTHREAD_MUTEX_INITIALIZER;

void kr_runtime_init(int argc, char **argv) {
    saved_argc = argc;
    saved_argv = argv;
    if (setlocale(LC_CTYPE, "C.UTF-8") == NULL) {
        setlocale(LC_CTYPE, "");
    }
}

void kr_runtime_exit(void) {
    fflush(stdout);
}

KrArray *kr_runtime_args(const KrType *array_type) {
    int64_t count = saved_argc > 1 ? saved_argc - 1 : 0;
    KrArray *array = kr_alloc((int64_t)sizeof(KrArray) + count * (int64_t)sizeof(void *));
    array->type = array_type;
    array->len = count;
    KrString **out = (KrString **)array->data;
    for (int64_t i = 0; i < count; i++) {
        out[i] = kr_string_new(saved_argv[i + 1], (int64_t)strlen(saved_argv[i + 1]));
    }
    return array;
}

void kr_console_print(const KrString *string) {
    pthread_mutex_lock(&console_lock);
    fwrite(string->data, 1, (size_t)string->len, stdout);
    pthread_mutex_unlock(&console_lock);
}

void kr_console_newline(void) {
    pthread_mutex_lock(&console_lock);
    fputc('\n', stdout);
    pthread_mutex_unlock(&console_lock);
}

KrString *kr_console_read_line(void) {
    fflush(stdout);
    char *line = NULL;
    size_t cap = 0;
    ssize_t len = getline(&line, &cap, stdin);
    if (len < 0) {
        free(line);
        return NULL;
    }
    while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r')) {
        len--;
    }
    KrString *string = kr_string_new(line, len);
    free(line);
    return string;
}

void kr_uncaught(const KrType *type, const KrString *message) {
    fflush(stdout);
    if (message == NULL) {
        fprintf(stderr, "Exception in thread \"main\" %s\n", type->name);
    } else {
        fprintf(stderr, "Exception in thread \"main\" %s: %.*s\n", type->name,
                (int)message->len, message->data);
    }
}

void kr_report_thread(const KrType *type, const KrString *message) {
    fflush(stdout);
    if (message == NULL) {
        fprintf(stderr, "Exception in thread %s\n", type->name);
    } else {
        fprintf(stderr, "Exception in thread %s: %.*s\n", type->name, (int)message->len,
                message->data);
    }
}

bool kr_char_is_digit(int32_t c) {
    return c < 128 ? c >= '0' && c <= '9' : iswdigit((wint_t)c) != 0;
}

bool kr_char_is_letter(int32_t c) {
    return c < 128 ? (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
                   : iswalpha((wint_t)c) != 0;
}

bool kr_char_is_whitespace(int32_t c) {
    return c < 128 ? c == ' ' || (c >= 9 && c <= 13) : iswspace((wint_t)c) != 0;
}

bool kr_char_is_upper(int32_t c) {
    return c < 128 ? c >= 'A' && c <= 'Z' : iswupper((wint_t)c) != 0;
}

bool kr_char_is_lower(int32_t c) {
    return c < 128 ? c >= 'a' && c <= 'z' : iswlower((wint_t)c) != 0;
}

int32_t kr_char_to_upper(int32_t c) {
    if (c < 128) {
        return c >= 'a' && c <= 'z' ? c - 32 : c;
    }
    return (int32_t)towupper((wint_t)c);
}

int32_t kr_char_to_lower(int32_t c) {
    if (c < 128) {
        return c >= 'A' && c <= 'Z' ? c + 32 : c;
    }
    return (int32_t)towlower((wint_t)c);
}

static _Thread_local uint64_t random_state;

double kr_random(void) {
    if (random_state == 0) {
        struct timespec now;
        clock_gettime(CLOCK_REALTIME, &now);
        random_state = ((uint64_t)now.tv_nsec << 20) ^ (uint64_t)now.tv_sec ^
                       (uint64_t)(uintptr_t)&random_state;
        random_state |= 1;
    }
    random_state ^= random_state >> 12;
    random_state ^= random_state << 25;
    random_state ^= random_state >> 27;
    uint64_t bits = random_state * 2685821657736338717ULL;
    return (double)(bits >> 11) * (1.0 / 9007199254740992.0);
}

typedef struct {
    void (*entry)(void *);
    void *argument;
} ThreadStart;

static void *run_thread(void *pointer) {
    ThreadStart start = *(ThreadStart *)pointer;
    kr_free(pointer);
    start.entry(start.argument);
    return NULL;
}

void *kr_thread_spawn(void (*entry)(void *), void *argument) {
    ThreadStart *start = kr_alloc(sizeof(ThreadStart));
    start->entry = entry;
    start->argument = argument;
    pthread_t *thread = kr_alloc(sizeof(pthread_t));
    if (pthread_create(thread, NULL, run_thread, start) != 0) {
        kr_panic("failed to spawn a thread");
    }
    return thread;
}

void kr_thread_join(void *handle) {
    if (handle == NULL) {
        return;
    }
    pthread_join(*(pthread_t *)handle, NULL);
    kr_free(handle);
}

void kr_thread_sleep(int64_t millis) {
    if (millis <= 0) {
        return;
    }
    struct timespec duration = {millis / 1000, (millis % 1000) * 1000000};
    while (nanosleep(&duration, &duration) != 0) {
    }
}

void *kr_mutex_new(void) {
    pthread_mutex_t *mutex = kr_alloc(sizeof(pthread_mutex_t));
    pthread_mutex_init(mutex, NULL);
    return mutex;
}

void kr_mutex_lock(void *mutex) {
    pthread_mutex_lock(mutex);
}

void kr_mutex_unlock(void *mutex) {
    pthread_mutex_unlock(mutex);
}

void kr_mutex_free(void *mutex) {
    if (mutex == NULL) {
        return;
    }
    pthread_mutex_destroy(mutex);
    kr_free(mutex);
}
