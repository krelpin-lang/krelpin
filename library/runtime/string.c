#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "krelpin.h"

static int64_t count_chars(const char *bytes, int64_t len) {
    int64_t chars = 0;
    for (int64_t i = 0; i < len; i++) {
        if (((unsigned char)bytes[i] & 0xC0) != 0x80) {
            chars++;
        }
    }
    return chars;
}

static int32_t decode(const unsigned char *bytes, int64_t len, int64_t *at) {
    unsigned char lead = bytes[*at];
    int extra = lead < 0x80 ? 0 : lead < 0xE0 ? 1 : lead < 0xF0 ? 2 : 3;
    int32_t c = extra == 0   ? lead
                : extra == 1 ? lead & 0x1F
                : extra == 2 ? lead & 0x0F
                             : lead & 0x07;
    (*at)++;
    for (int k = 0; k < extra && *at < len; k++) {
        c = (c << 6) | (bytes[*at] & 0x3F);
        (*at)++;
    }
    return c;
}

static int encode(int32_t c, char *out) {
    if (c < 0 || c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) {
        c = 0xFFFD;
    }
    if (c < 0x80) {
        out[0] = (char)c;
        return 1;
    }
    if (c < 0x800) {
        out[0] = (char)(0xC0 | (c >> 6));
        out[1] = (char)(0x80 | (c & 0x3F));
        return 2;
    }
    if (c < 0x10000) {
        out[0] = (char)(0xE0 | (c >> 12));
        out[1] = (char)(0x80 | ((c >> 6) & 0x3F));
        out[2] = (char)(0x80 | (c & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | (c >> 18));
    out[1] = (char)(0x80 | ((c >> 12) & 0x3F));
    out[2] = (char)(0x80 | ((c >> 6) & 0x3F));
    out[3] = (char)(0x80 | (c & 0x3F));
    return 4;
}

static KrString *allocate(int64_t len) {
    KrString *string = kr_alloc((int64_t)sizeof(KrString) + len + 1);
    string->type = &kr_string_type;
    atomic_store_explicit(&string->refs, 1, memory_order_relaxed);
    string->len = len;
    return string;
}

KrString *kr_string_new(const char *bytes, int64_t len) {
    KrString *string = allocate(len);
    memcpy(string->data, bytes, (size_t)len);
    string->chars = count_chars(bytes, len);
    return string;
}

static KrString *from_c(const char *text) {
    return kr_string_new(text, (int64_t)strlen(text));
}

void kr_string_retain(KrString *string) {
    if (string != NULL && atomic_load_explicit(&string->refs, memory_order_relaxed) >= 0) {
        atomic_fetch_add_explicit(&string->refs, 1, memory_order_relaxed);
    }
}

void kr_string_release(void *pointer) {
    KrString *string = pointer;
    if (string == NULL || atomic_load_explicit(&string->refs, memory_order_relaxed) < 0) {
        return;
    }
    if (atomic_fetch_sub_explicit(&string->refs, 1, memory_order_release) == 1) {
        atomic_thread_fence(memory_order_acquire);
        kr_free(string);
    }
}

KrString *kr_string_concat(const KrString *a, const KrString *b) {
    KrString *string = allocate(a->len + b->len);
    memcpy(string->data, a->data, (size_t)a->len);
    memcpy(string->data + a->len, b->data, (size_t)b->len);
    string->chars = a->chars + b->chars;
    return string;
}

int32_t kr_string_length(const KrString *string) {
    return (int32_t)string->chars;
}

int64_t kr_string_byte_offset(const KrString *string, int64_t chars) {
    if (string->chars == string->len) {
        return chars;
    }
    int64_t at = 0;
    for (int64_t seen = 0; seen < chars && at < string->len; seen++) {
        decode((const unsigned char *)string->data, string->len, &at);
    }
    return at;
}

KrArray *kr_string_chars(const KrString *string, const KrType *array_type) {
    KrArray *array = kr_alloc((int64_t)sizeof(KrArray) + string->chars * 4);
    array->type = array_type;
    array->len = string->chars;
    int32_t *out = (int32_t *)array->data;
    int64_t at = 0;
    for (int64_t i = 0; i < string->chars; i++) {
        out[i] = decode((const unsigned char *)string->data, string->len, &at);
    }
    return array;
}

KrString *kr_string_substring(const KrString *string, int32_t begin, int32_t end) {
    if (begin < 0 || end < begin || end > string->chars) {
        return NULL;
    }
    int64_t from = kr_string_byte_offset(string, begin);
    int64_t to = kr_string_byte_offset(string, end);
    return kr_string_new(string->data + from, to - from);
}

static int64_t find_bytes(const KrString *string, const KrString *text, int64_t from) {
    if (text->len == 0) {
        return from;
    }
    for (int64_t i = from; i + text->len <= string->len; i++) {
        if (memcmp(string->data + i, text->data, (size_t)text->len) == 0) {
            return i;
        }
    }
    return -1;
}

int32_t kr_string_index_of(const KrString *string, const KrString *text) {
    int64_t at = find_bytes(string, text, 0);
    if (at < 0) {
        return -1;
    }
    return (int32_t)count_chars(string->data, at);
}

bool kr_string_starts_with(const KrString *string, const KrString *prefix) {
    return prefix->len <= string->len &&
           memcmp(string->data, prefix->data, (size_t)prefix->len) == 0;
}

bool kr_string_ends_with(const KrString *string, const KrString *suffix) {
    return suffix->len <= string->len &&
           memcmp(string->data + string->len - suffix->len, suffix->data,
                  (size_t)suffix->len) == 0;
}

static KrString *map_chars(const KrString *string, int32_t (*map)(int32_t)) {
    char *buffer = kr_alloc(string->len * 2 + 4);
    int64_t len = 0;
    int64_t at = 0;
    int64_t cap = string->len * 2 + 4;
    while (at < string->len) {
        int32_t c = map(decode((const unsigned char *)string->data, string->len, &at));
        if (len + 4 > cap) {
            cap *= 2;
            buffer = kr_realloc(buffer, cap);
        }
        len += encode(c, buffer + len);
    }
    KrString *result = kr_string_new(buffer, len);
    kr_free(buffer);
    return result;
}

KrString *kr_string_to_upper(const KrString *string) {
    return map_chars(string, kr_char_to_upper);
}

KrString *kr_string_to_lower(const KrString *string) {
    return map_chars(string, kr_char_to_lower);
}

KrString *kr_string_trim(const KrString *string) {
    int64_t begin = 0;
    int64_t end = string->len;
    while (begin < end && (unsigned char)string->data[begin] <= ' ') {
        begin++;
    }
    while (end > begin && (unsigned char)string->data[end - 1] <= ' ') {
        end--;
    }
    return kr_string_new(string->data + begin, end - begin);
}

KrString *kr_string_trim_start(const KrString *string) {
    int64_t begin = 0;
    while (begin < string->len && (unsigned char)string->data[begin] <= ' ') {
        begin++;
    }
    return kr_string_new(string->data + begin, string->len - begin);
}

KrString *kr_string_trim_end(const KrString *string) {
    int64_t end = string->len;
    while (end > 0 && (unsigned char)string->data[end - 1] <= ' ') {
        end--;
    }
    return kr_string_new(string->data, end);
}

int32_t kr_string_char_at(const KrString *string, int32_t index) {
    if (index < 0 || index >= string->chars) {
        return -1;
    }
    int64_t at = kr_string_byte_offset(string, index);
    return decode((const unsigned char *)string->data, string->len, &at);
}

int32_t kr_string_index_of_from(const KrString *string, const KrString *text, int32_t from) {
    if (from < 0) {
        from = 0;
    }
    if (from > string->chars) {
        return -1;
    }
    int64_t at = find_bytes(string, text, kr_string_byte_offset(string, from));
    if (at < 0) {
        return -1;
    }
    return (int32_t)count_chars(string->data, at);
}

int32_t kr_string_last_index_of(const KrString *string, const KrString *text) {
    for (int64_t i = string->len - text->len; i >= 0; i--) {
        if (memcmp(string->data + i, text->data, (size_t)text->len) == 0) {
            return (int32_t)count_chars(string->data, i);
        }
    }
    return -1;
}

KrString *kr_string_replace(const KrString *string, const KrString *target,
                            const KrString *replacement) {
    if (target->len == 0) {
        return kr_string_new(string->data, string->len);
    }
    int64_t cap = string->len + 16;
    char *buffer = kr_alloc(cap);
    int64_t len = 0;
    int64_t at = 0;
    for (;;) {
        int64_t found = find_bytes(string, target, at);
        int64_t stop = found < 0 ? string->len : found;
        int64_t needed = len + (stop - at) + (found < 0 ? 0 : replacement->len);
        if (needed > cap) {
            cap = needed * 2;
            buffer = kr_realloc(buffer, cap);
        }
        memcpy(buffer + len, string->data + at, (size_t)(stop - at));
        len += stop - at;
        if (found < 0) {
            break;
        }
        memcpy(buffer + len, replacement->data, (size_t)replacement->len);
        len += replacement->len;
        at = found + target->len;
    }
    KrString *result = kr_string_new(buffer, len);
    kr_free(buffer);
    return result;
}

KrString *kr_string_repeat(const KrString *string, int32_t count) {
    if (count < 0) {
        return NULL;
    }
    KrString *result = allocate(string->len * count);
    for (int32_t i = 0; i < count; i++) {
        memcpy(result->data + string->len * i, string->data, (size_t)string->len);
    }
    result->chars = string->chars * count;
    return result;
}

KrArray *kr_string_split(const KrString *string, const KrString *separator,
                         const KrType *array_type) {
    int64_t count = 1;
    if (separator->len == 0) {
        count = string->chars;
    } else {
        for (int64_t at = find_bytes(string, separator, 0); at >= 0;
             at = find_bytes(string, separator, at + separator->len)) {
            count++;
        }
    }
    KrArray *array = kr_alloc((int64_t)sizeof(KrArray) + count * (int64_t)sizeof(void *));
    array->type = array_type;
    array->len = count;
    KrString **out = (KrString **)array->data;
    if (separator->len == 0) {
        int64_t at = 0;
        for (int64_t i = 0; i < count; i++) {
            int64_t start = at;
            decode((const unsigned char *)string->data, string->len, &at);
            out[i] = kr_string_new(string->data + start, at - start);
        }
        return array;
    }
    int64_t start = 0;
    for (int64_t i = 0; i < count; i++) {
        int64_t found = find_bytes(string, separator, start);
        int64_t stop = found < 0 ? string->len : found;
        out[i] = kr_string_new(string->data + start, stop - start);
        start = stop + separator->len;
    }
    return array;
}

int32_t kr_string_compare(const KrString *a, const KrString *b) {
    int64_t shorter = a->len < b->len ? a->len : b->len;
    int order = memcmp(a->data, b->data, (size_t)shorter);
    if (order != 0) {
        return order < 0 ? -1 : 1;
    }
    return a->len == b->len ? 0 : a->len < b->len ? -1 : 1;
}

bool kr_string_equals(const KrString *a, const KrString *b) {
    return a == b || (a->len == b->len && memcmp(a->data, b->data, (size_t)a->len) == 0);
}

int32_t kr_string_hash(const KrString *string) {
    uint32_t hash = 0;
    int64_t at = 0;
    while (at < string->len) {
        hash = hash * 31 + (uint32_t)decode((const unsigned char *)string->data, string->len, &at);
    }
    return (int32_t)hash;
}

KrString *kr_string_join(const KrString *separator, const KrArray *parts) {
    KrString *const *items = (KrString *const *)parts->data;
    int64_t len = 0;
    int64_t chars = 0;
    for (int64_t i = 0; i < parts->len; i++) {
        len += items[i]->len + (i > 0 ? separator->len : 0);
        chars += items[i]->chars + (i > 0 ? separator->chars : 0);
    }
    KrString *result = allocate(len);
    int64_t at = 0;
    for (int64_t i = 0; i < parts->len; i++) {
        if (i > 0) {
            memcpy(result->data + at, separator->data, (size_t)separator->len);
            at += separator->len;
        }
        memcpy(result->data + at, items[i]->data, (size_t)items[i]->len);
        at += items[i]->len;
    }
    result->chars = chars;
    return result;
}

KrString *kr_string_from_long(int64_t value) {
    char buffer[32];
    snprintf(buffer, sizeof buffer, "%lld", (long long)value);
    return from_c(buffer);
}

static KrString *format_floating(double value, int max_digits) {
    if (isnan(value)) {
        return from_c("NaN");
    }
    if (isinf(value)) {
        return from_c(value > 0 ? "Infinity" : "-Infinity");
    }
    if (value == 0) {
        return from_c(signbit(value) ? "-0.0" : "0.0");
    }
    char digits[64];
    int precision = 0;
    for (; precision < max_digits; precision++) {
        snprintf(digits, sizeof digits, "%.*e", precision, value);
        double parsed = strtod(digits, NULL);
        if (max_digits <= 9 ? (float)parsed == (float)value : parsed == value) {
            break;
        }
    }
    char *exponent_mark = strchr(digits, 'e');
    int exponent = atoi(exponent_mark + 1);
    *exponent_mark = '\0';
    char mantissa[64];
    int count = 0;
    bool negative = digits[0] == '-';
    for (char *c = digits + (negative ? 1 : 0); *c != '\0'; c++) {
        if (*c != '.') {
            mantissa[count++] = *c;
        }
    }
    mantissa[count] = '\0';
    char out[96];
    int len = 0;
    if (negative) {
        out[len++] = '-';
    }
    double magnitude = fabs(value);
    if (magnitude >= 1e-3 && magnitude < 1e7) {
        if (exponent >= 0) {
            for (int i = 0; i <= exponent; i++) {
                out[len++] = i < count ? mantissa[i] : '0';
            }
            out[len++] = '.';
            if (exponent + 1 >= count) {
                out[len++] = '0';
            } else {
                for (int i = exponent + 1; i < count; i++) {
                    out[len++] = mantissa[i];
                }
            }
        } else {
            out[len++] = '0';
            out[len++] = '.';
            for (int i = 0; i < -exponent - 1; i++) {
                out[len++] = '0';
            }
            for (int i = 0; i < count; i++) {
                out[len++] = mantissa[i];
            }
        }
    } else {
        out[len++] = mantissa[0];
        out[len++] = '.';
        if (count == 1) {
            out[len++] = '0';
        }
        for (int i = 1; i < count; i++) {
            out[len++] = mantissa[i];
        }
        len += snprintf(out + len, sizeof out - (size_t)len, "E%d", exponent);
    }
    return kr_string_new(out, len);
}

KrString *kr_string_from_double(double value) {
    return format_floating(value, 17);
}

KrString *kr_string_from_float(float value) {
    return format_floating(value, 9);
}

KrString *kr_string_from_char(int32_t value) {
    char buffer[4];
    int len = encode(value, buffer);
    return kr_string_new(buffer, len);
}

KrString *kr_string_from_bool(bool value) {
    return from_c(value ? "true" : "false");
}

KrString *kr_string_describe(const void *object) {
    char buffer[256];
    const KrType *type = ((const KrObject *)object)->type;
    snprintf(buffer, sizeof buffer, "%s@%llx", type->name,
             (unsigned long long)(uintptr_t)object);
    return from_c(buffer);
}

static char *terminated(const KrString *text) {
    char *buffer = kr_alloc(text->len + 1);
    memcpy(buffer, text->data, (size_t)text->len);
    return buffer;
}

bool kr_parse_long(const KrString *text, int64_t min, int64_t max, int64_t *out) {
    if (text->len == 0) {
        return false;
    }
    int64_t at = 0;
    bool negative = false;
    if (text->data[0] == '-' || text->data[0] == '+') {
        negative = text->data[0] == '-';
        at = 1;
    }
    if (at == text->len) {
        return false;
    }
    uint64_t limit = negative ? (uint64_t)(-(min + 1)) + 1 : (uint64_t)max;
    uint64_t value = 0;
    for (; at < text->len; at++) {
        char c = text->data[at];
        if (c < '0' || c > '9') {
            return false;
        }
        uint64_t digit = (uint64_t)(c - '0');
        if (value > (limit - digit) / 10) {
            return false;
        }
        value = value * 10 + digit;
    }
    *out = negative ? (int64_t)(0 - value) : (int64_t)value;
    return true;
}

bool kr_parse_double(const KrString *text, double *out) {
    if (text->len == 0) {
        return false;
    }
    char *buffer = terminated(text);
    char *end = NULL;
    double value = strtod(buffer, &end);
    bool ok = end == buffer + text->len;
    kr_free(buffer);
    if (ok) {
        *out = value;
    }
    return ok;
}

KrString *kr_string_bounds(int64_t index, int64_t len) {
    char buffer[96];
    snprintf(buffer, sizeof buffer, "Index %lld out of bounds for length %lld", (long long)index,
             (long long)len);
    return from_c(buffer);
}

KrString *kr_string_cast(const void *object, const KrString *target) {
    const KrType *type = ((const KrObject *)object)->type;
    char buffer[512];
    snprintf(buffer, sizeof buffer, "%s cannot be cast to %.*s", type->name, (int)target->len,
             target->data);
    return from_c(buffer);
}

KrString *kr_string_number_format(const KrString *text) {
    KrString *prefix = from_c("For input string: \"");
    KrString *quote = from_c("\"");
    KrString *head = kr_string_concat(prefix, text);
    KrString *message = kr_string_concat(head, quote);
    kr_string_release(prefix);
    kr_string_release(quote);
    kr_string_release(head);
    return message;
}
