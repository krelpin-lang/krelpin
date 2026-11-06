/* TCP and UDP sockets for `std.net`.
 *
 * Each socket object holds a pointer to a `Socket` in its native slot. The
 * intrinsics receive the address of that slot, so the ones that open a
 * socket can store it there; dropping the object calls `kr_net_close`.
 *
 * A socket blocks unless `kr_net_set_nonblocking` turned that off. The
 * `try` functions are meant for non-blocking sockets: they return at once
 * and tell "not ready yet" apart from failure, so they can be driven by a
 * poller (see poll.c).
 *
 * Every call retries when a signal interrupts it and reports failures
 * through the error that `Files.lastError` describes. Writing to a
 * connection the peer has closed fails with EPIPE instead of killing the
 * process with SIGPIPE. */

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#include "krelpin.h"

#if defined(MSG_NOSIGNAL)
#define SEND_FLAGS MSG_NOSIGNAL
#else
#define SEND_FLAGS 0
#endif

#define MAX_ADDRESS 1024
#define READ_CHUNK 8192

/* What the `try` functions return. */
enum {
    TRY_FAILED = -1,
    TRY_NOT_READY = 0,
    TRY_DONE = 1,
};

typedef struct {
    char *data;
    int64_t start;
    int64_t len;
    int64_t cap;
} Buffer;

typedef struct {
    int fd;
    bool nonblocking;
    /* Set once a read has seen the end of the stream. */
    bool eof;
    /* Bytes received but not handed out yet, kept by `readLine`. */
    Buffer in;
    /* Bytes queued by `kr_tcp_stream_queue_*` and not sent yet. */
    Buffer out;
    /* The sender of the last datagram a UDP socket received. */
    struct sockaddr_storage sender;
    socklen_t sender_len;
} Socket;

static Socket *socket_of(void **slot) {
    return (Socket *)*slot;
}

static int64_t buffered(const Socket *socket) {
    return socket->in.len - socket->in.start;
}

static bool would_block(void) {
    return errno == EAGAIN || errno == EWOULDBLOCK;
}

void kr_net_close(void *handle) {
    Socket *socket = handle;
    if (socket == NULL) {
        return;
    }
    if (socket->fd >= 0) {
        kr_task_forget_fd(socket->fd);
        close(socket->fd);
    }
    kr_free(socket->in.data);
    kr_free(socket->out.data);
    kr_free(socket);
}

/* Replaces whatever the slot held with a socket around `fd`. */
static void adopt(void **slot, int fd) {
    kr_net_close(*slot);
    Socket *socket = kr_alloc((int64_t)sizeof(Socket));
    memset(socket, 0, sizeof *socket);
    socket->fd = fd;
    *slot = socket;
}

/* Sets the flags every socket gets: closed in child processes, and no
 * SIGPIPE where the platform does that per socket. */
static void configure(int fd) {
    int flags = fcntl(fd, F_GETFD);
    if (flags >= 0) {
        fcntl(fd, F_SETFD, flags | FD_CLOEXEC);
    }
#if defined(SO_NOSIGPIPE)
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, sizeof on);
#endif
}

static bool set_nonblocking(int fd, bool on) {
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0) {
        return false;
    }
    int wanted = on ? flags | O_NONBLOCK : flags & ~O_NONBLOCK;
    return wanted == flags || fcntl(fd, F_SETFL, wanted) == 0;
}

/* EAGAIN means the socket isn't ready when it is non-blocking, and that a
 * timeout set with SO_RCVTIMEO or SO_SNDTIMEO passed when it blocks. */
static bool fail_io(const Socket *socket) {
    if (would_block()) {
        return socket->nonblocking ? kr_io_fail_would_block() : kr_io_fail(ETIMEDOUT);
    }
    return kr_io_fail_errno();
}

/* Splits `host:port` or `[host]:port` and resolves it. */
static struct addrinfo *resolve(const KrString *address, int type, bool passive) {
    if (address->len == 0 || address->len >= MAX_ADDRESS ||
        memchr(address->data, 0, (size_t)address->len) != NULL) {
        kr_io_fail_bad_address();
        return NULL;
    }
    char text[MAX_ADDRESS];
    memcpy(text, address->data, (size_t)address->len);
    text[address->len] = 0;

    char *host = text;
    char *colon;
    if (text[0] == '[') {
        char *close = strchr(text, ']');
        if (close == NULL || close[1] != ':') {
            kr_io_fail_bad_address();
            return NULL;
        }
        *close = 0;
        host = text + 1;
        colon = close + 1;
    } else {
        colon = strrchr(text, ':');
        if (colon == NULL || memchr(text, ':', (size_t)(colon - text)) != NULL) {
            /* No port, or an IPv6 address without brackets. */
            kr_io_fail_bad_address();
            return NULL;
        }
    }
    *colon = 0;
    const char *port = colon + 1;
    size_t digits = strlen(port);
    if (digits == 0 || digits > 5 || strspn(port, "0123456789") != digits || atoi(port) > 65535) {
        kr_io_fail_bad_address();
        return NULL;
    }

    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = type;
    hints.ai_flags = AI_NUMERICSERV | (passive ? AI_PASSIVE : 0);
    struct addrinfo *found = NULL;
    int status = getaddrinfo(host[0] != 0 ? host : NULL, port, &hints, &found);
    if (status == EAI_SYSTEM) {
        kr_io_fail_errno();
        return NULL;
    }
    if (status != 0) {
        kr_io_fail_resolve(status);
        return NULL;
    }
    return found;
}

/* `ip:port`, with brackets around an IPv6 address. */
static KrString *describe(const struct sockaddr *address, socklen_t len) {
    char host[NI_MAXHOST];
    char port[NI_MAXSERV];
    int status = getnameinfo(address, len, host, sizeof host, port, sizeof port,
                             NI_NUMERICHOST | NI_NUMERICSERV);
    if (status != 0) {
        kr_io_fail_resolve(status);
        return NULL;
    }
    char text[NI_MAXHOST + NI_MAXSERV + 4];
    int written = address->sa_family == AF_INET6
                      ? snprintf(text, sizeof text, "[%s]:%s", host, port)
                      : snprintf(text, sizeof text, "%s:%s", host, port);
    return kr_string_new(text, written);
}

/* Opens a socket for the first address that works. With `bind_to`, binds
 * it (and listens for TCP); otherwise connects it. */
static int open_socket(const KrString *address, int type, bool bind_to) {
    struct addrinfo *found = resolve(address, type, bind_to);
    if (found == NULL) {
        return -1;
    }
    int error = 0;
    int fd = -1;
    for (struct addrinfo *at = found; at != NULL; at = at->ai_next) {
        fd = socket(at->ai_family, at->ai_socktype, at->ai_protocol);
        if (fd < 0) {
            error = errno;
            continue;
        }
        configure(fd);
        int status;
        if (bind_to) {
            if (type == SOCK_STREAM) {
                /* Like Rust: a restarted server can take its port back at once. */
                int on = 1;
                setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);
            }
            status = bind(fd, at->ai_addr, at->ai_addrlen);
            if (status == 0 && type == SOCK_STREAM) {
                status = listen(fd, 128);
            }
        } else {
            do {
                status = connect(fd, at->ai_addr, at->ai_addrlen);
            } while (status != 0 && errno == EINTR);
        }
        if (status == 0) {
            break;
        }
        error = errno;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(found);
    if (fd < 0) {
        kr_io_fail(error);
    }
    return fd;
}

KrString *kr_net_local_addr(void **slot) {
    struct sockaddr_storage address;
    socklen_t len = sizeof address;
    if (getsockname(socket_of(slot)->fd, (struct sockaddr *)&address, &len) != 0) {
        kr_io_fail_errno();
        return NULL;
    }
    return describe((struct sockaddr *)&address, len);
}

bool kr_net_set_nonblocking(void **slot, bool on) {
    Socket *socket = socket_of(slot);
    if (!set_nonblocking(socket->fd, on)) {
        return kr_io_fail_errno();
    }
    socket->nonblocking = on;
    return true;
}

bool kr_net_set_timeout(void **slot, int64_t millis, bool writing) {
    if (millis < 0) {
        return kr_io_fail(EINVAL);
    }
    struct timeval timeout;
    timeout.tv_sec = (time_t)(millis / 1000);
    timeout.tv_usec = (suseconds_t)((millis % 1000) * 1000);
    int option = writing ? SO_SNDTIMEO : SO_RCVTIMEO;
    if (setsockopt(socket_of(slot)->fd, SOL_SOCKET, option, &timeout, sizeof timeout) != 0) {
        return kr_io_fail_errno();
    }
    return true;
}

/* TCP listeners. */

bool kr_tcp_listener_bind(void **slot, const KrString *address) {
    int fd = open_socket(address, SOCK_STREAM, true);
    if (fd < 0) {
        return false;
    }
    adopt(slot, fd);
    return true;
}

/* TCP streams. */

bool kr_tcp_stream_connect(void **slot, const KrString *address) {
    int fd = open_socket(address, SOCK_STREAM, false);
    if (fd < 0) {
        return false;
    }
    adopt(slot, fd);
    return true;
}

/* Accepts a connection whose socket blocks or not as `nonblocking` says;
 * on BSD and macOS it would otherwise take the listener's mode. */
static int accept_from(Socket *listener, bool nonblocking) {
    int fd;
    do {
        fd = accept(listener->fd, NULL, NULL);
    } while (fd < 0 && errno == EINTR);
    if (fd < 0) {
        return -1;
    }
    configure(fd);
    if (!set_nonblocking(fd, nonblocking)) {
        int error = errno;
        close(fd);
        errno = error;
        return -1;
    }
    return fd;
}

bool kr_tcp_stream_accept(void **slot, void *listener) {
    int fd = accept_from(listener, false);
    if (fd < 0) {
        return fail_io(listener);
    }
    adopt(slot, fd);
    return true;
}

int32_t kr_tcp_stream_try_accept(void **slot, void *listener) {
    int fd = accept_from(listener, true);
    if (fd < 0) {
        if (would_block() || errno == ECONNABORTED) {
            return TRY_NOT_READY;
        }
        kr_io_fail_errno();
        return TRY_FAILED;
    }
    adopt(slot, fd);
    socket_of(slot)->nonblocking = true;
    return TRY_DONE;
}

bool kr_tcp_stream_start_connect(void **slot, const KrString *address) {
    struct addrinfo *found = resolve(address, SOCK_STREAM, false);
    if (found == NULL) {
        return false;
    }
    int error = 0;
    int fd = -1;
    for (struct addrinfo *at = found; at != NULL; at = at->ai_next) {
        fd = socket(at->ai_family, at->ai_socktype, at->ai_protocol);
        if (fd < 0) {
            error = errno;
            continue;
        }
        configure(fd);
        int status = set_nonblocking(fd, true) ? 0 : -1;
        if (status == 0) {
            do {
                status = connect(fd, at->ai_addr, at->ai_addrlen);
            } while (status != 0 && errno == EINTR);
        }
        /* EINTR leaves the connection going on, as EINPROGRESS does. */
        if (status == 0 || errno == EINPROGRESS || errno == EINTR) {
            break;
        }
        error = errno;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(found);
    if (fd < 0) {
        return kr_io_fail(error);
    }
    adopt(slot, fd);
    socket_of(slot)->nonblocking = true;
    return true;
}

int32_t kr_tcp_stream_finish_connect(void **slot) {
    int fd = socket_of(slot)->fd;
    int error = 0;
    socklen_t len = sizeof error;
    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &len) != 0) {
        kr_io_fail_errno();
        return TRY_FAILED;
    }
    if (error != 0) {
        kr_io_fail(error);
        return TRY_FAILED;
    }
    struct sockaddr_storage peer;
    len = sizeof peer;
    if (getpeername(fd, (struct sockaddr *)&peer, &len) == 0) {
        return TRY_DONE;
    }
    if (errno == ENOTCONN || errno == EINVAL) {
        return TRY_NOT_READY;
    }
    kr_io_fail_errno();
    return TRY_FAILED;
}

KrString *kr_tcp_stream_peer_addr(void **slot) {
    struct sockaddr_storage address;
    socklen_t len = sizeof address;
    if (getpeername(socket_of(slot)->fd, (struct sockaddr *)&address, &len) != 0) {
        kr_io_fail_errno();
        return NULL;
    }
    return describe((struct sockaddr *)&address, len);
}

bool kr_tcp_stream_set_no_delay(void **slot, bool on) {
    int value = on ? 1 : 0;
    if (setsockopt(socket_of(slot)->fd, IPPROTO_TCP, TCP_NODELAY, &value, sizeof value) != 0) {
        return kr_io_fail_errno();
    }
    return true;
}

/* Receives up to `max` bytes into `out`: 0 at the end of the stream, -1 on
 * failure. */
static int64_t receive(Socket *socket, char *out, int64_t max) {
    ssize_t got;
    do {
        got = recv(socket->fd, out, (size_t)max, 0);
    } while (got < 0 && errno == EINTR);
    if (got < 0) {
        fail_io(socket);
        return -1;
    }
    if (got == 0) {
        socket->eof = true;
    }
    return (int64_t)got;
}

/* Makes room for `more` bytes after the ones in `buffer`. */
static void reserve(Buffer *buffer, int64_t more) {
    if (buffer->start > 0) {
        memmove(buffer->data, buffer->data + buffer->start,
                (size_t)(buffer->len - buffer->start));
        buffer->len -= buffer->start;
        buffer->start = 0;
    }
    if (buffer->len + more > buffer->cap) {
        int64_t cap = buffer->cap > 0 ? buffer->cap : READ_CHUNK;
        while (cap < buffer->len + more) {
            cap *= 2;
        }
        buffer->data = kr_realloc(buffer->data, cap);
        buffer->cap = cap;
    }
}

/* The complete line at the front of the buffer, searching from byte
 * `searched` on. Sets `*found` when there is one; the line is NULL then if
 * it isn't UTF-8. */
static KrString *take_line(Socket *socket, int64_t searched, bool *found) {
    char *from = socket->in.data + socket->in.start;
    char *newline = memchr(from + searched, '\n', (size_t)(buffered(socket) - searched));
    *found = newline != NULL;
    if (newline == NULL) {
        return NULL;
    }
    int64_t len = newline - from;
    int64_t consumed = len + 1;
    if (len > 0 && from[len - 1] == '\r') {
        len--;
    }
    KrString *line = NULL;
    if (kr_utf8_valid(from, len)) {
        line = kr_string_new(from, len);
    } else {
        kr_io_fail_not_utf8();
    }
    socket->in.start += consumed;
    return line;
}

/* Everything buffered, as the last line of a stream that has ended. */
static KrString *take_rest(Socket *socket) {
    int64_t len = buffered(socket);
    if (len == 0) {
        return NULL;
    }
    const char *from = socket->in.data + socket->in.start;
    socket->in.start = socket->in.len;
    if (!kr_utf8_valid(from, len)) {
        kr_io_fail_not_utf8();
        return NULL;
    }
    return kr_string_new(from, len);
}

/* Reads lines for both `readLine` and `tryReadLine`. A non-blocking socket
 * that runs out of data returns NULL with no error, keeping what it has
 * read. */
static KrString *read_line(Socket *socket) {
    kr_io_clear_error();
    int64_t searched = 0;
    for (;;) {
        bool found;
        KrString *line = take_line(socket, searched, &found);
        if (found) {
            return line;
        }
        if (socket->eof) {
            /* The last line may have no line ending. */
            return take_rest(socket);
        }
        searched = buffered(socket);
        reserve(&socket->in, READ_CHUNK);
        int64_t got = receive(socket, socket->in.data + socket->in.len, READ_CHUNK);
        if (got < 0) {
            if (socket->nonblocking && would_block()) {
                kr_io_clear_error();
            }
            return NULL;
        }
        socket->in.len += got;
    }
}

KrString *kr_tcp_stream_read_line(void **slot) {
    Socket *socket = socket_of(slot);
    if (socket->nonblocking) {
        /* `readLine` promises a line or the end, so it can't stop early. */
        KrString *line = read_line(socket);
        if (line == NULL && !kr_io_failed() && !socket->eof) {
            kr_io_fail_would_block();
        }
        return line;
    }
    return read_line(socket);
}

KrString *kr_tcp_stream_try_read_line(void **slot) {
    return read_line(socket_of(slot));
}

bool kr_tcp_stream_at_end(void **slot) {
    Socket *socket = socket_of(slot);
    return socket->eof && buffered(socket) == 0;
}

KrString *kr_tcp_stream_read_all(void **slot) {
    Socket *socket = socket_of(slot);
    while (!socket->eof) {
        reserve(&socket->in, READ_CHUNK);
        int64_t got = receive(socket, socket->in.data + socket->in.len, READ_CHUNK);
        if (got < 0) {
            return NULL;
        }
        socket->in.len += got;
    }
    const char *from = socket->in.data + socket->in.start;
    int64_t len = buffered(socket);
    socket->in.start = socket->in.len;
    if (!kr_utf8_valid(from, len)) {
        kr_io_fail_not_utf8();
        return NULL;
    }
    return kr_string_new(from, len);
}

static KrArray *new_bytes(const char *data, int64_t len, const KrType *array_type) {
    KrArray *array = kr_alloc((int64_t)sizeof(KrArray) + len);
    array->type = array_type;
    array->len = len;
    if (len > 0) {
        memcpy(array->data, data, (size_t)len);
    }
    return array;
}

KrArray *kr_tcp_stream_read(void **slot, int32_t max, const KrType *array_type) {
    Socket *socket = socket_of(slot);
    kr_io_clear_error();
    if (buffered(socket) > 0) {
        int64_t len = buffered(socket) < max ? buffered(socket) : max;
        KrArray *bytes = new_bytes(socket->in.data + socket->in.start, len, array_type);
        socket->in.start += len;
        return bytes;
    }
    if (socket->eof) {
        return new_bytes(NULL, 0, array_type);
    }
    char chunk[READ_CHUNK];
    int64_t want = max < READ_CHUNK ? max : READ_CHUNK;
    int64_t got = receive(socket, chunk, want);
    if (got < 0) {
        return NULL;
    }
    return new_bytes(chunk, got, array_type);
}

KrArray *kr_tcp_stream_try_read(void **slot, int32_t max, const KrType *array_type) {
    KrArray *bytes = kr_tcp_stream_read(slot, max, array_type);
    if (bytes == NULL && socket_of(slot)->nonblocking && would_block()) {
        kr_io_clear_error();
    }
    return bytes;
}

/* Sends what it can of `data` without blocking on a non-blocking socket:
 * the count sent, or -1 on failure with `errno` set. */
static int64_t send_some(Socket *socket, const char *data, int64_t len) {
    int64_t total = 0;
    while (total < len) {
        ssize_t sent = send(socket->fd, data + total, (size_t)(len - total), SEND_FLAGS);
        if (sent < 0) {
            if (errno == EINTR) {
                continue;
            }
            if (total > 0 && would_block()) {
                break;
            }
            return -1;
        }
        total += sent;
    }
    return total;
}

static bool send_all(Socket *socket, const char *data, int64_t len) {
    int64_t sent = send_some(socket, data, len);
    if (sent < 0) {
        return fail_io(socket);
    }
    if (sent < len) {
        /* The socket stopped taking data part of the way through. */
        return socket->nonblocking ? kr_io_fail_would_block() : kr_io_fail(ETIMEDOUT);
    }
    return true;
}

bool kr_tcp_stream_write(void **slot, const KrString *text) {
    return send_all(socket_of(slot), text->data, text->len);
}

bool kr_tcp_stream_write_bytes(void **slot, const KrArray *bytes) {
    return send_all(socket_of(slot), (const char *)bytes->data, bytes->len);
}

int32_t kr_tcp_stream_try_write(void **slot, const KrArray *bytes, int32_t offset) {
    Socket *socket = socket_of(slot);
    int64_t sent = send_some(socket, (const char *)bytes->data + offset, bytes->len - offset);
    if (sent < 0) {
        if (would_block()) {
            return 0;
        }
        fail_io(socket);
        return -1;
    }
    return (int32_t)sent;
}

static void queue(Socket *socket, const char *data, int64_t len) {
    reserve(&socket->out, len);
    memcpy(socket->out.data + socket->out.len, data, (size_t)len);
    socket->out.len += len;
}

void kr_tcp_stream_queue_text(void **slot, const KrString *text) {
    queue(socket_of(slot), text->data, text->len);
}

void kr_tcp_stream_queue_bytes(void **slot, const KrArray *bytes) {
    queue(socket_of(slot), (const char *)bytes->data, bytes->len);
}

int32_t kr_tcp_stream_flush(void **slot) {
    Socket *socket = socket_of(slot);
    Buffer *out = &socket->out;
    int64_t sent = send_some(socket, out->data + out->start, out->len - out->start);
    if (sent < 0) {
        if (would_block()) {
            return TRY_NOT_READY;
        }
        fail_io(socket);
        return TRY_FAILED;
    }
    out->start += sent;
    if (out->start < out->len) {
        return TRY_NOT_READY;
    }
    out->start = 0;
    out->len = 0;
    return TRY_DONE;
}

int64_t kr_tcp_stream_queued(void **slot) {
    const Buffer *out = &socket_of(slot)->out;
    return out->len - out->start;
}

bool kr_tcp_stream_shutdown_write(void **slot) {
    if (shutdown(socket_of(slot)->fd, SHUT_WR) != 0) {
        return kr_io_fail_errno();
    }
    return true;
}

/* UDP sockets. */

bool kr_udp_bind(void **slot, const KrString *address) {
    int fd = open_socket(address, SOCK_DGRAM, true);
    if (fd < 0) {
        return false;
    }
    adopt(slot, fd);
    return true;
}

/* Sends one datagram: TRY_NOT_READY only on a non-blocking socket. */
static int32_t send_datagram(void **slot, const KrArray *bytes, const KrString *address) {
    struct addrinfo *found = resolve(address, SOCK_DGRAM, false);
    if (found == NULL) {
        return TRY_FAILED;
    }
    Socket *socket = socket_of(slot);
    int fd = socket->fd;
    int error = EAFNOSUPPORT;
    bool sent = false;
    /* The socket was bound for one address family; use the first match. */
    for (struct addrinfo *at = found; at != NULL && !sent; at = at->ai_next) {
        ssize_t count;
        do {
            count = sendto(fd, bytes->data, (size_t)bytes->len, SEND_FLAGS, at->ai_addr,
                           at->ai_addrlen);
        } while (count < 0 && errno == EINTR);
        if (count >= 0) {
            sent = true;
        } else {
            error = errno;
            if (error == EAGAIN || error == EWOULDBLOCK) {
                break;
            }
        }
    }
    freeaddrinfo(found);
    if (sent) {
        return TRY_DONE;
    }
    if (error == EAGAIN || error == EWOULDBLOCK) {
        if (socket->nonblocking) {
            return TRY_NOT_READY;
        }
        error = ETIMEDOUT;
    }
    kr_io_fail(error);
    return TRY_FAILED;
}

bool kr_udp_send_to(void **slot, const KrArray *bytes, const KrString *address) {
    switch (send_datagram(slot, bytes, address)) {
    case TRY_DONE:
        return true;
    case TRY_NOT_READY:
        return kr_io_fail_would_block();
    default:
        return false;
    }
}

int32_t kr_udp_try_send_to(void **slot, const KrArray *bytes, const KrString *address) {
    return send_datagram(slot, bytes, address);
}

KrArray *kr_udp_receive(void **slot, int32_t max, const KrType *array_type) {
    Socket *socket = socket_of(slot);
    char *chunk = kr_alloc(max);
    socket->sender_len = sizeof socket->sender;
    ssize_t got;
    do {
        got = recvfrom(socket->fd, chunk, (size_t)max, 0, (struct sockaddr *)&socket->sender,
                       &socket->sender_len);
    } while (got < 0 && errno == EINTR);
    if (got < 0) {
        kr_free(chunk);
        fail_io(socket);
        return NULL;
    }
    KrArray *bytes = new_bytes(chunk, got, array_type);
    kr_free(chunk);
    return bytes;
}

KrArray *kr_udp_try_receive(void **slot, int32_t max, const KrType *array_type) {
    kr_io_clear_error();
    KrArray *bytes = kr_udp_receive(slot, max, array_type);
    if (bytes == NULL && socket_of(slot)->nonblocking && would_block()) {
        kr_io_clear_error();
    }
    return bytes;
}

/* The descriptor a poller watches; see poll.c. */
int kr_net_fd(void *handle) {
    return ((Socket *)handle)->fd;
}

KrString *kr_udp_last_sender(void **slot) {
    Socket *socket = socket_of(slot);
    return describe((struct sockaddr *)&socket->sender, socket->sender_len);
}

/* Bytes and text. */

KrArray *kr_string_bytes(const KrString *string, const KrType *array_type) {
    return new_bytes(string->data, string->len, array_type);
}

KrString *kr_string_from_utf8(const KrArray *bytes) {
    if (!kr_utf8_valid((const char *)bytes->data, bytes->len)) {
        return NULL;
    }
    return kr_string_new((const char *)bytes->data, bytes->len);
}
