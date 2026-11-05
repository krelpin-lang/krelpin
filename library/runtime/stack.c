/* Reports stack overflows instead of dying with a bare SIGSEGV.
 *
 * Each thread records where its stack ends and gets an alternate signal
 * stack. When a SIGSEGV or SIGBUS hits just below the end of the stack, the
 * handler names the thread and aborts; any other fault is re-raised with the
 * default action so it still produces a normal crash. Only executables
 * install the handler: a library must not take over its host's signals. */

#define _GNU_SOURCE

#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "krelpin.h"

/* How far below the end of the stack a fault still counts as an overflow.
 * A frame larger than this skips the guard pages and is reported as a crash. */
#define GUARD_WINDOW (256 * 1024)

static _Atomic bool installed;
static _Thread_local uintptr_t stack_low;
static _Thread_local void *alternate_stack;
static _Thread_local const char *thread_name;

static uintptr_t current_stack_low(void) {
#if defined(__APPLE__)
    pthread_t self = pthread_self();
    uintptr_t top = (uintptr_t)pthread_get_stackaddr_np(self);
    return top - (uintptr_t)pthread_get_stacksize_np(self);
#else
    pthread_attr_t attr;
    void *low = NULL;
    size_t size = 0;
    if (pthread_getattr_np(pthread_self(), &attr) != 0) {
        return 0;
    }
    pthread_attr_getstack(&attr, &low, &size);
    pthread_attr_destroy(&attr);
    return (uintptr_t)low;
#endif
}

static void write_all(const char *text) {
    size_t len = strlen(text);
    while (len > 0) {
        ssize_t written = write(STDERR_FILENO, text, len);
        if (written <= 0) {
            return;
        }
        text += written;
        len -= (size_t)written;
    }
}

static void on_fault(int signal, siginfo_t *info, void *context) {
    (void)context;
    uintptr_t address = (uintptr_t)info->si_addr;
    uintptr_t page = (uintptr_t)sysconf(_SC_PAGESIZE);
    if (stack_low != 0 && address < stack_low + page && address + GUARD_WINDOW >= stack_low) {
        write_all("\nthread '");
        write_all(thread_name != NULL ? thread_name : "<unnamed>");
        write_all("' has overflowed its stack\nfatal runtime error: stack overflow\n");
        abort();
    }
    /* Not an overflow: crash the way the program would have without us. */
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_handler = SIG_DFL;
    sigemptyset(&action.sa_mask);
    sigaction(signal, &action, NULL);
}

static void prepare_thread(const char *name) {
    thread_name = name;
    stack_low = current_stack_low();
    size_t size = (size_t)SIGSTKSZ > 65536 ? (size_t)SIGSTKSZ : 65536;
    void *memory = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (memory == MAP_FAILED) {
        return;
    }
    stack_t stack;
    memset(&stack, 0, sizeof stack);
    stack.ss_sp = memory;
    stack.ss_size = size;
    if (sigaltstack(&stack, NULL) != 0) {
        munmap(memory, size);
        return;
    }
    alternate_stack = memory;
}

void kr_stack_guard_install(void) {
    prepare_thread("main");
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = on_fault;
    action.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&action.sa_mask);
    sigaction(SIGSEGV, &action, NULL);
    sigaction(SIGBUS, &action, NULL);
    installed = true;
}

void kr_stack_guard_thread_start(void) {
    if (installed) {
        prepare_thread("<unnamed>");
    }
}

void kr_stack_guard_thread_end(void) {
    if (alternate_stack == NULL) {
        return;
    }
    stack_t stack;
    memset(&stack, 0, sizeof stack);
    stack.ss_flags = SS_DISABLE;
    sigaltstack(&stack, NULL);
    size_t size = (size_t)SIGSTKSZ > 65536 ? (size_t)SIGSTKSZ : 65536;
    munmap(alternate_stack, size);
    alternate_stack = NULL;
}
