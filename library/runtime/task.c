/* The reactor behind `std.task.Runtime`.
 *
 * Each thread that runs `Runtime.blockOn` has one reactor. It knows which
 * task waits for which socket and which timer, keeps the queue of tasks
 * that were woken, and owns the futures of spawned tasks between polls.
 * Task 0 is the future `blockOn` was given; spawned tasks get ids from 1.
 *
 * Sockets are watched one-shot: a task that waits for a socket is woken
 * once, the next time the socket is ready, and has to ask again if it
 * still can't make progress. */

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "krelpin.h"

#if defined(__linux__)
#define USE_EPOLL 1
#include <sys/epoll.h>
#elif defined(__APPLE__) || defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__) || \
    defined(__DragonFly__)
#define USE_KQUEUE 1
#include <sys/event.h>
#include <sys/time.h>
#endif

enum {
    INTEREST_READABLE = 1,
    INTEREST_WRITABLE = 2,
};

#define MAX_EVENTS 256

typedef struct {
    int32_t reader;
    int32_t writer;
    bool added;
} Waiters;

typedef struct {
    int64_t deadline;
    int32_t task;
} Timer;

typedef struct {
    int fd;
    /* By file descriptor: the tasks that wait for it. */
    Waiters *waiters;
    int32_t waiter_cap;
    int32_t waiting;
    Timer *timers;
    int32_t timer_count;
    int32_t timer_cap;
    int32_t *woken;
    int32_t woken_head;
    int32_t woken_count;
    int32_t woken_cap;
    /* By task: whether it is in `woken`, and the futures of spawned tasks.
     * A spawned task's slot is NULL while it is being polled. */
    bool *queued;
    void **futures;
    bool *live;
    int32_t task_cap;
    int32_t live_count;
} Reactor;

static _Thread_local Reactor *current;

static void *grow(void *items, int32_t *cap, int32_t needed, size_t size) {
    if (needed <= *cap) {
        return items;
    }
    int32_t next = *cap == 0 ? 16 : *cap;
    while (next < needed) {
        next *= 2;
    }
    items = kr_realloc(items, (int64_t)(next * size));
    memset((char *)items + *cap * size, 0, (next - *cap) * size);
    *cap = next;
    return items;
}

static void ensure_task(Reactor *reactor, int32_t task) {
    if (task < reactor->task_cap) {
        return;
    }
    int32_t cap = reactor->task_cap;
    reactor->queued = grow(reactor->queued, &cap, task + 1, sizeof(bool));
    cap = reactor->task_cap;
    reactor->futures = grow(reactor->futures, &cap, task + 1, sizeof(void *));
    cap = reactor->task_cap;
    reactor->live = grow(reactor->live, &cap, task + 1, sizeof(bool));
    reactor->task_cap = cap;
}

static Waiters *waiters_of(Reactor *reactor, int fd) {
    int32_t old = reactor->waiter_cap;
    reactor->waiters = grow(reactor->waiters, &reactor->waiter_cap, fd + 1, sizeof(Waiters));
    for (int32_t i = old; i < reactor->waiter_cap; i++) {
        reactor->waiters[i].reader = -1;
        reactor->waiters[i].writer = -1;
    }
    return &reactor->waiters[fd];
}

int64_t kr_task_now(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

void kr_task_wake(int32_t task) {
    Reactor *reactor = current;
    if (reactor == NULL || task < 0) {
        return;
    }
    ensure_task(reactor, task);
    if (reactor->queued[task]) {
        return;
    }
    reactor->queued[task] = true;
    reactor->woken = grow(reactor->woken, &reactor->woken_cap, reactor->woken_count + 1, sizeof(int32_t));
    reactor->woken[reactor->woken_count++] = task;
}

int32_t kr_task_next_woken(void) {
    Reactor *reactor = current;
    if (reactor == NULL) {
        return -1;
    }
    if (reactor->woken_head == reactor->woken_count) {
        reactor->woken_head = 0;
        reactor->woken_count = 0;
        return -1;
    }
    int32_t task = reactor->woken[reactor->woken_head++];
    reactor->queued[task] = false;
    return task;
}

void kr_task_sleep_until(int64_t deadline, int32_t task) {
    Reactor *reactor = current;
    if (reactor == NULL) {
        return;
    }
    reactor->timers =
        grow(reactor->timers, &reactor->timer_cap, reactor->timer_count + 1, sizeof(Timer));
    reactor->timers[reactor->timer_count].deadline = deadline;
    reactor->timers[reactor->timer_count].task = task;
    reactor->timer_count++;
}

/* Wakes the tasks whose timers have run out. */
static void fire_timers(Reactor *reactor) {
    int64_t now = kr_task_now();
    int32_t i = 0;
    while (i < reactor->timer_count) {
        if (reactor->timers[i].deadline <= now) {
            kr_task_wake(reactor->timers[i].task);
            reactor->timers[i] = reactor->timers[--reactor->timer_count];
        } else {
            i++;
        }
    }
}

/* How long to wait for sockets: not at all if a task is woken already,
 * until the next timer if there is one, else forever (-1). */
static int64_t wait_millis(Reactor *reactor) {
    if (reactor->woken_head < reactor->woken_count) {
        return 0;
    }
    int64_t wait = -1;
    int64_t now = kr_task_now();
    for (int32_t i = 0; i < reactor->timer_count; i++) {
        int64_t left = reactor->timers[i].deadline - now;
        left = left < 0 ? 0 : left;
        if (wait < 0 || left < wait) {
            wait = left;
        }
    }
    return wait;
}

static void wake_waiter(Reactor *reactor, int32_t *task) {
    if (*task >= 0) {
        kr_task_wake(*task);
        *task = -1;
        reactor->waiting--;
    }
}

#if defined(USE_EPOLL)

static int create_poller(void) {
    return epoll_create1(EPOLL_CLOEXEC);
}

/* Watches `fd` for what its waiters want, once. */
static bool arm(Reactor *reactor, int fd) {
    Waiters *waiters = &reactor->waiters[fd];
    struct epoll_event event;
    memset(&event, 0, sizeof event);
    event.events = EPOLLONESHOT;
    if (waiters->reader >= 0) {
        event.events |= EPOLLIN | EPOLLRDHUP;
    }
    if (waiters->writer >= 0) {
        event.events |= EPOLLOUT;
    }
    if (event.events == EPOLLONESHOT) {
        return true;
    }
    event.data.fd = fd;
    int op = waiters->added ? EPOLL_CTL_MOD : EPOLL_CTL_ADD;
    if (epoll_ctl(reactor->fd, op, fd, &event) != 0) {
        /* The descriptor was closed and reused since it was added, or
         * added by an earlier reactor. */
        op = errno == ENOENT ? EPOLL_CTL_ADD : errno == EEXIST ? EPOLL_CTL_MOD : -1;
        if (op < 0 || epoll_ctl(reactor->fd, op, fd, &event) != 0) {
            return false;
        }
    }
    waiters->added = true;
    return true;
}

static bool wait_events(Reactor *reactor, int64_t millis) {
    struct epoll_event events[MAX_EVENTS];
    int timeout = millis < 0 ? -1 : millis > 1000000000 ? 1000000000 : (int)millis;
    int count = epoll_wait(reactor->fd, events, MAX_EVENTS, timeout);
    if (count < 0) {
        return errno == EINTR;
    }
    for (int i = 0; i < count; i++) {
        int fd = events[i].data.fd;
        uint32_t flags = events[i].events;
        Waiters *waiters = &reactor->waiters[fd];
        if (flags & (EPOLLIN | EPOLLRDHUP | EPOLLHUP | EPOLLERR)) {
            wake_waiter(reactor, &waiters->reader);
        }
        if (flags & (EPOLLOUT | EPOLLHUP | EPOLLERR)) {
            wake_waiter(reactor, &waiters->writer);
        }
        arm(reactor, fd);
    }
    return true;
}

#elif defined(USE_KQUEUE)

static int create_poller(void) {
    return kqueue();
}

static bool arm(Reactor *reactor, int fd) {
    Waiters *waiters = &reactor->waiters[fd];
    struct kevent changes[2];
    int count = 0;
    if (waiters->reader >= 0) {
        EV_SET(&changes[count++], fd, EVFILT_READ, EV_ADD | EV_ONESHOT, 0, 0, NULL);
    }
    if (waiters->writer >= 0) {
        EV_SET(&changes[count++], fd, EVFILT_WRITE, EV_ADD | EV_ONESHOT, 0, 0, NULL);
    }
    return count == 0 || kevent(reactor->fd, changes, count, NULL, 0, NULL) == 0;
}

static bool wait_events(Reactor *reactor, int64_t millis) {
    struct kevent events[MAX_EVENTS];
    struct timespec timeout;
    timeout.tv_sec = millis / 1000;
    timeout.tv_nsec = (millis % 1000) * 1000000;
    int count = kevent(reactor->fd, NULL, 0, events, MAX_EVENTS, millis < 0 ? NULL : &timeout);
    if (count < 0) {
        return errno == EINTR;
    }
    for (int i = 0; i < count; i++) {
        int fd = (int)events[i].ident;
        if (fd >= reactor->waiter_cap) {
            continue;
        }
        Waiters *waiters = &reactor->waiters[fd];
        if (events[i].filter == EVFILT_READ) {
            wake_waiter(reactor, &waiters->reader);
        } else if (events[i].filter == EVFILT_WRITE) {
            wake_waiter(reactor, &waiters->writer);
        }
    }
    return true;
}

#else

static int create_poller(void) {
    errno = ENOSYS;
    return -1;
}

static bool arm(Reactor *reactor, int fd) {
    (void)reactor;
    (void)fd;
    errno = ENOSYS;
    return false;
}

static bool wait_events(Reactor *reactor, int64_t millis) {
    (void)reactor;
    if (millis > 0) {
        struct timespec pause = {millis / 1000, (millis % 1000) * 1000000};
        nanosleep(&pause, NULL);
    }
    return true;
}

#endif

/* Makes `task` wake when `fd` is ready for `interest`. */
bool kr_task_wait_fd(int fd, int32_t interest, int32_t task) {
    Reactor *reactor = current;
    if (reactor == NULL) {
        return kr_io_fail(EINVAL);
    }
    if (fd < 0) {
        return kr_io_fail(EBADF);
    }
    Waiters *waiters = waiters_of(reactor, fd);
    if ((interest & INTEREST_READABLE) && waiters->reader < 0) {
        reactor->waiting++;
    }
    if ((interest & INTEREST_WRITABLE) && waiters->writer < 0) {
        reactor->waiting++;
    }
    if (interest & INTEREST_READABLE) {
        waiters->reader = task;
    }
    if (interest & INTEREST_WRITABLE) {
        waiters->writer = task;
    }
    if (!arm(reactor, fd)) {
        int error = errno;
        wake_waiter(reactor, &waiters->reader);
        wake_waiter(reactor, &waiters->writer);
        return kr_io_fail(error);
    }
    return true;
}

/* Forgets the waiters of a socket that is being closed, so a later socket
 * that gets the same descriptor starts clean. */
void kr_task_forget_fd(int fd) {
    Reactor *reactor = current;
    if (reactor == NULL || fd < 0 || fd >= reactor->waiter_cap) {
        return;
    }
    Waiters *waiters = &reactor->waiters[fd];
    wake_waiter(reactor, &waiters->reader);
    wake_waiter(reactor, &waiters->writer);
    waiters->added = false;
}

/* Waits until a task can make progress. Returns false if no task can
 * ever be woken: nothing is woken, and no task waits for a socket or a
 * timer. */
bool kr_task_turn(void) {
    Reactor *reactor = current;
    if (reactor == NULL) {
        return false;
    }
    int64_t wait = wait_millis(reactor);
    if (wait < 0 && reactor->waiting == 0) {
        return false;
    }
    if (reactor->waiting > 0 || wait > 0) {
        if (!wait_events(reactor, wait)) {
            return false;
        }
    }
    fire_timers(reactor);
    return true;
}

bool kr_task_enter(void) {
    if (current != NULL) {
        return false;
    }
    int fd = create_poller();
    if (fd < 0) {
        return false;
    }
    int flags = fcntl(fd, F_GETFD);
    if (flags >= 0) {
        fcntl(fd, F_SETFD, flags | FD_CLOEXEC);
    }
    Reactor *reactor = kr_alloc((int64_t)sizeof(Reactor));
    memset(reactor, 0, sizeof *reactor);
    reactor->fd = fd;
    current = reactor;
    return true;
}

/* Drops the spawned tasks that never finished and closes the reactor. */
void kr_task_exit(void) {
    Reactor *reactor = current;
    if (reactor == NULL) {
        return;
    }
    /* A future's drop may spawn or wake; those see no reactor. */
    current = NULL;
    for (int32_t task = 0; task < reactor->task_cap; task++) {
        if (reactor->futures[task] != NULL) {
            kr_drop_object(reactor->futures[task]);
        }
    }
    close(reactor->fd);
    kr_free(reactor->waiters);
    kr_free(reactor->timers);
    kr_free(reactor->woken);
    kr_free(reactor->queued);
    kr_free(reactor->futures);
    kr_free(reactor->live);
    kr_free(reactor);
}

/* Takes ownership of `future` and returns its task, which is woken. */
int32_t kr_task_spawn(void *future) {
    Reactor *reactor = current;
    if (reactor == NULL) {
        kr_drop_object(future);
        return -1;
    }
    int32_t task = 1;
    while (task < reactor->task_cap && reactor->live[task]) {
        task++;
    }
    ensure_task(reactor, task);
    reactor->live[task] = true;
    reactor->futures[task] = future;
    reactor->live_count++;
    kr_task_wake(task);
    return task;
}

/* Hands the future of a spawned task to the caller, which polls it and
 * gives it back with `kr_task_put` or ends the task with
 * `kr_task_finish`. Returns NULL if the task has ended. */
void *kr_task_take(int32_t task) {
    Reactor *reactor = current;
    if (reactor == NULL || task <= 0 || task >= reactor->task_cap) {
        return NULL;
    }
    void *future = reactor->futures[task];
    reactor->futures[task] = NULL;
    return future;
}

void kr_task_put(int32_t task, void *future) {
    Reactor *reactor = current;
    if (reactor == NULL || task <= 0 || task >= reactor->task_cap || !reactor->live[task]) {
        kr_drop_object(future);
        return;
    }
    reactor->futures[task] = future;
}

void kr_task_finish(int32_t task) {
    Reactor *reactor = current;
    if (reactor == NULL || task <= 0 || task >= reactor->task_cap || !reactor->live[task]) {
        return;
    }
    reactor->live[task] = false;
    reactor->live_count--;
}

int32_t kr_task_live(void) {
    return current == NULL ? 0 : current->live_count;
}

/* The descriptor of the socket in `slot`, for `kr_task_wait_fd`. */
int32_t kr_net_fd_of(void **slot) {
    return *slot == NULL ? -1 : kr_net_fd(*slot);
}
