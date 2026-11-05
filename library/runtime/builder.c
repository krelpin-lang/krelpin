#include <string.h>

#include "krelpin.h"

static void reserve(KrBuilder *builder, int64_t extra) {
    int64_t needed = builder->len + extra;
    if (needed <= builder->cap) {
        return;
    }
    int64_t cap = builder->cap < 16 ? 16 : builder->cap;
    while (cap < needed) {
        cap *= 2;
    }
    builder->data = kr_realloc(builder->data, cap);
    builder->cap = cap;
}


static int64_t byte_offset(const KrBuilder *builder, int64_t chars) {
    int64_t at = 0;
    int64_t seen = 0;
    while (at < builder->len && seen < chars) {
        at++;
        while (at < builder->len && ((unsigned char)builder->data[at] & 0xC0) == 0x80) {
            at++;
        }
        seen++;
    }
    return at;
}

void kr_builder_append(KrBuilder *builder, const KrString *text) {
    reserve(builder, text->len);
    memcpy(builder->data + builder->len, text->data, (size_t)text->len);
    builder->len += text->len;
    builder->chars += text->chars;
}

bool kr_builder_insert(KrBuilder *builder, int32_t index, const KrString *text) {
    if (index < 0 || index > builder->chars) {
        return false;
    }
    int64_t at = byte_offset(builder, index);
    reserve(builder, text->len);
    memmove(builder->data + at + text->len, builder->data + at, (size_t)(builder->len - at));
    memcpy(builder->data + at, text->data, (size_t)text->len);
    builder->len += text->len;
    builder->chars += text->chars;
    return true;
}

bool kr_builder_truncate(KrBuilder *builder, int32_t length) {
    if (length < 0 || length > builder->chars) {
        return false;
    }
    builder->len = byte_offset(builder, length);
    builder->chars = length;
    return true;
}

void kr_builder_reverse(KrBuilder *builder) {
    char *reversed = kr_alloc(builder->len + 1);
    int64_t at = 0;
    int64_t out = builder->len;
    while (at < builder->len) {
        int64_t start = at;
        at++;
        while (at < builder->len && ((unsigned char)builder->data[at] & 0xC0) == 0x80) {
            at++;
        }
        out -= at - start;
        memcpy(reversed + out, builder->data + start, (size_t)(at - start));
    }
    if (builder->len > 0) {
        memcpy(builder->data, reversed, (size_t)builder->len);
    }
    kr_free(reversed);
}

KrString *kr_builder_to_string(const KrBuilder *builder) {
    return kr_string_new(builder->data, builder->len);
}

int32_t kr_builder_length(const KrBuilder *builder) {
    return (int32_t)builder->chars;
}

void kr_builder_clear(KrBuilder *builder) {
    builder->len = 0;
    builder->chars = 0;
}

void kr_builder_free(KrBuilder *builder) {
    kr_free(builder->data);
}
