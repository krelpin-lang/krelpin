#define _GNU_SOURCE

#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "krelpin.h"

static FILE *bridge_in;
static FILE *bridge_out;

void kr_bridge_open(void) {
    if (bridge_out != NULL) {
        return;
    }
    fflush(stdout);
    int in = dup(STDIN_FILENO);
    int out = dup(STDOUT_FILENO);
    if (in < 0 || out < 0) {
        kr_panic("couldn't open the macro bridge");
    }
    fcntl(in, F_SETFD, FD_CLOEXEC);
    fcntl(out, F_SETFD, FD_CLOEXEC);
    int null = open("/dev/null", O_RDONLY);
    if (null >= 0) {
        dup2(null, STDIN_FILENO);
        close(null);
    }
    dup2(STDERR_FILENO, STDOUT_FILENO);
    bridge_in = fdopen(in, "r");
    bridge_out = fdopen(out, "w");
    if (bridge_in == NULL || bridge_out == NULL) {
        kr_panic("couldn't open the macro bridge");
    }
}

KrString *kr_bridge_receive(void) {
    if (bridge_in == NULL) {
        return NULL;
    }
    char *line = NULL;
    size_t cap = 0;
    ssize_t len = getline(&line, &cap, bridge_in);
    if (len < 0) {
        free(line);
        return NULL;
    }
    if (len > 0 && line[len - 1] == '\n') {
        len--;
    }
    KrString *string = kr_string_new(line, len);
    free(line);
    return string;
}

static void send_bytes(const char *bytes, size_t len) {
    if (bridge_out == NULL) {
        return;
    }
    fwrite(bytes, 1, len, bridge_out);
    fputc('\n', bridge_out);
    fflush(bridge_out);
}

void kr_bridge_send(const KrString *line) {
    send_bytes(line->data, (size_t)line->len);
}

static char *escape_bytes(const char *bytes, int64_t len, size_t *out_len) {
    char *out = kr_alloc(len * 2 + 1);
    size_t at = 0;
    for (int64_t i = 0; i < len; i++) {
        char c = bytes[i];
        const char *replacement = NULL;
        switch (c) {
        case '\\':
            replacement = "\\\\";
            break;
        case '\n':
            replacement = "\\n";
            break;
        case '\t':
            replacement = "\\t";
            break;
        case '\r':
            replacement = "\\r";
            break;
        default:
            out[at++] = c;
            continue;
        }
        out[at++] = replacement[0];
        out[at++] = replacement[1];
    }
    *out_len = at;
    return out;
}

KrString *kr_bridge_escape(const KrString *text) {
    size_t len;
    char *escaped = escape_bytes(text->data, text->len, &len);
    KrString *string = kr_string_new(escaped, (int64_t)len);
    kr_free(escaped);
    return string;
}

KrString *kr_bridge_unescape(const KrString *text) {
    char *out = kr_alloc(text->len + 1);
    int64_t at = 0;
    for (int64_t i = 0; i < text->len; i++) {
        char c = text->data[i];
        if (c == '\\' && i + 1 < text->len) {
            i++;
            switch (text->data[i]) {
            case 'n':
                c = '\n';
                break;
            case 't':
                c = '\t';
                break;
            case 'r':
                c = '\r';
                break;
            default:
                c = text->data[i];
                break;
            }
        }
        out[at++] = c;
    }
    KrString *string = kr_string_new(out, at);
    kr_free(out);
    return string;
}

bool kr_bridge_report(const KrType *type, const KrString *message) {
    if (bridge_out == NULL) {
        return false;
    }
    size_t name_len = strlen(type->name);
    int64_t message_len = message == NULL ? 0 : message->len;
    size_t total = name_len + (message == NULL ? 0 : 2 + (size_t)message_len);
    char *text = kr_alloc((int64_t)total + 1);
    memcpy(text, type->name, name_len);
    if (message != NULL) {
        memcpy(text + name_len, ": ", 2);
        memcpy(text + name_len + 2, message->data, (size_t)message_len);
    }
    size_t escaped_len;
    char *escaped = escape_bytes(text, (int64_t)total, &escaped_len);
    char *line = kr_alloc((int64_t)escaped_len + 7);
    memcpy(line, "panic\t", 6);
    memcpy(line + 6, escaped, escaped_len);
    send_bytes(line, escaped_len + 6);
    kr_free(line);
    kr_free(escaped);
    kr_free(text);
    return true;
}
