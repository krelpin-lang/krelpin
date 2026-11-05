/* TCP and UDP sockets for `std.net`.
 *
 * Each socket object holds a pointer to a `Socket` in its native slot. The
 * intrinsics receive the address of that slot, so the ones that open a
 * socket can store it there; dropping the object calls `kr_net_close`.
 *
 * Every call blocks, retries when a signal interrupts it, and reports
 * failures through the error that `Files.lastError` describes. Writing to a
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

typedef struct {
    int fd;
    /* Bytes received but not handed out yet, kept by `readLine`. */
    char *buffer;
    int64_t start;
    int64_t len;
    int64_t cap;
    /* The sender of the last datagram a UDP socket received. */
    struct sockaddr_storage sender;
    socklen_t sender_len;
} Socket;

static Socket *socket_of(void **slot) {
    return (Socket *)*slot;
}

static int64_t buffered(const Socket *socket) {
    return socket->len - socket->start;
}

void kr_net_close(void *handle) {
    Socket *socket = handle;
    if (socket == NULL) {
        return;
    }
    if (socket->fd >= 0) {
        close(socket->fd);
    }
    kr_free(socket->buffer);
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

/* A timeout set with SO_RCVTIMEO or SO_SNDTIMEO shows up as EAGAIN. */
static bool fail_io(void) {
    if (errno == EAGAIN || errno == EWOULDBLOCK) {
        return kr_io_fail(ETIMEDOUT);
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

bool kr_tcp_stream_accept(void **slot, void *listener) {
    int fd;
    do {
        fd = accept(((Socket *)listener)->fd, NULL, NULL);
    } while (fd < 0 && errno == EINTR);
    if (fd < 0) {
        return fail_io();
    }
    configure(fd);
    adopt(slot, fd);
    return true;
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
        fail_io();
        return -1;
    }
    return (int64_t)got;
}

/* Makes room for `more` bytes after the buffered ones. */
static void reserve(Socket *socket, int64_t more) {
    if (socket->start > 0) {
        memmove(socket->buffer, socket->buffer + socket->start, (size_t)buffered(socket));
        socket->len -= socket->start;
        socket->start = 0;
    }
    if (socket->len + more > socket->cap) {
        int64_t cap = socket->cap > 0 ? socket->cap : READ_CHUNK;
        while (cap < socket->len + more) {
            cap *= 2;
        }
        socket->buffer = kr_realloc(socket->buffer, cap);
        socket->cap = cap;
    }
}

KrString *kr_tcp_stream_read_line(void **slot) {
    Socket *socket = socket_of(slot);
    kr_io_clear_error();
    int64_t searched = 0;
    for (;;) {
        char *from = socket->buffer + socket->start;
        char *newline = memchr(from + searched, '\n', (size_t)(buffered(socket) - searched));
        if (newline != NULL) {
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
            socket->start += consumed;
            return line;
        }
        searched = buffered(socket);
        reserve(socket, READ_CHUNK);
        int64_t got = receive(socket, socket->buffer + socket->len, READ_CHUNK);
        if (got < 0) {
            return NULL;
        }
        if (got == 0) {
            /* The last line may have no line ending. */
            int64_t len = buffered(socket);
            if (len == 0) {
                return NULL;
            }
            from = socket->buffer + socket->start;
            socket->start = socket->len;
            if (!kr_utf8_valid(from, len)) {
                kr_io_fail_not_utf8();
                return NULL;
            }
            return kr_string_new(from, len);
        }
        socket->len += got;
    }
}

KrString *kr_tcp_stream_read_all(void **slot) {
    Socket *socket = socket_of(slot);
    for (;;) {
        reserve(socket, READ_CHUNK);
        int64_t got = receive(socket, socket->buffer + socket->len, READ_CHUNK);
        if (got < 0) {
            return NULL;
        }
        if (got == 0) {
            break;
        }
        socket->len += got;
    }
    const char *from = socket->buffer + socket->start;
    int64_t len = buffered(socket);
    socket->start = socket->len;
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
    memcpy(array->data, data, (size_t)len);
    return array;
}

KrArray *kr_tcp_stream_read(void **slot, int32_t max, const KrType *array_type) {
    Socket *socket = socket_of(slot);
    if (buffered(socket) > 0) {
        int64_t len = buffered(socket) < max ? buffered(socket) : max;
        KrArray *bytes = new_bytes(socket->buffer + socket->start, len, array_type);
        socket->start += len;
        return bytes;
    }
    char chunk[READ_CHUNK];
    int64_t want = max < READ_CHUNK ? max : READ_CHUNK;
    int64_t got = receive(socket, chunk, want);
    if (got < 0) {
        return NULL;
    }
    return new_bytes(chunk, got, array_type);
}

static bool send_all(int fd, const char *data, int64_t len) {
    while (len > 0) {
        ssize_t sent = send(fd, data, (size_t)len, SEND_FLAGS);
        if (sent < 0) {
            if (errno == EINTR) {
                continue;
            }
            return fail_io();
        }
        data += sent;
        len -= sent;
    }
    return true;
}

bool kr_tcp_stream_write(void **slot, const KrString *text) {
    return send_all(socket_of(slot)->fd, text->data, text->len);
}

bool kr_tcp_stream_write_bytes(void **slot, const KrArray *bytes) {
    return send_all(socket_of(slot)->fd, (const char *)bytes->data, bytes->len);
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

bool kr_udp_send_to(void **slot, const KrArray *bytes, const KrString *address) {
    struct addrinfo *found = resolve(address, SOCK_DGRAM, false);
    if (found == NULL) {
        return false;
    }
    int fd = socket_of(slot)->fd;
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
        }
    }
    freeaddrinfo(found);
    return sent || kr_io_fail(error);
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
        fail_io();
        return NULL;
    }
    KrArray *bytes = new_bytes(chunk, got, array_type);
    kr_free(chunk);
    return bytes;
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
