#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "krelpin.h"

typedef struct {
    const char *fill;
    int64_t fill_len;
    char align;
    bool plus;
    bool alternate;
    bool zero;
    int64_t width;
    int64_t precision;
    char kind;
} Spec;

static int64_t utf8_len(const char *bytes, int64_t len) {
    int64_t count = 0;
    for (int64_t i = 0; i < len; i++) {
        if (((unsigned char)bytes[i] & 0xC0) != 0x80) {
            count++;
        }
    }
    return count;
}

static int64_t utf8_char_len(unsigned char lead) {
    if (lead < 0x80) {
        return 1;
    }
    if ((lead & 0xE0) == 0xC0) {
        return 2;
    }
    if ((lead & 0xF0) == 0xE0) {
        return 3;
    }
    return 4;
}

static bool is_align(char c) {
    return c == '<' || c == '^' || c == '>';
}

static bool read_number(const char *text, int64_t len, int64_t *at, int64_t *out) {
    int64_t start = *at;
    int64_t value = 0;
    while (*at < len && text[*at] >= '0' && text[*at] <= '9') {
        value = value * 10 + (text[*at] - '0');
        if (value > 4096) {
            return false;
        }
        (*at)++;
    }
    if (*at == start) {
        return false;
    }
    *out = value;
    return true;
}

static bool parse_spec(const KrString *text, Spec *spec) {
    const char *s = text->data;
    int64_t len = text->len;
    memset(spec, 0, sizeof(*spec));
    spec->fill = " ";
    spec->fill_len = 1;
    spec->width = -1;
    spec->precision = -1;
    int64_t at = 0;
    if (len > 0) {
        int64_t first = utf8_char_len((unsigned char)s[0]);
        if (first < len && is_align(s[first])) {
            spec->fill = s;
            spec->fill_len = first;
            spec->align = s[first];
            at = first + 1;
        } else if (is_align(s[0])) {
            spec->align = s[0];
            at = 1;
        }
    }
    if (at < len && s[at] == '+') {
        spec->plus = true;
        at++;
    }
    if (at < len && s[at] == '#') {
        spec->alternate = true;
        at++;
    }
    if (at < len && s[at] == '0') {
        spec->zero = true;
        at++;
    }
    if (at < len && s[at] >= '0' && s[at] <= '9' && !read_number(s, len, &at, &spec->width)) {
        return false;
    }
    if (at < len && s[at] == '.') {
        at++;
        if (!read_number(s, len, &at, &spec->precision)) {
            return false;
        }
    }
    if (at < len) {
        char kind = s[at];
        if (kind != 'x' && kind != 'X' && kind != 'b' && kind != 'o' && kind != 'e' &&
            kind != 'E') {
            return false;
        }
        spec->kind = kind;
        at++;
    }
    return at == len;
}

static KrString *pad(const Spec *spec, const char *sign, const char *body, int64_t body_len,
                     char default_align, bool numeric) {
    int64_t sign_len = (int64_t)strlen(sign);
    int64_t chars = utf8_len(sign, sign_len) + utf8_len(body, body_len);
    int64_t missing = spec->width > chars ? spec->width - chars : 0;
    bool zero = numeric && spec->zero && spec->align == 0;
    int64_t fill_len = zero ? 1 : spec->fill_len;
    const char *fill = zero ? "0" : spec->fill;
    int64_t total = sign_len + body_len + missing * fill_len;
    char *out = kr_alloc(total + 1);
    int64_t at = 0;
    char align = spec->align != 0 ? spec->align : default_align;
    int64_t before = 0;
    if (zero) {
        before = 0;
    } else if (align == '>') {
        before = missing;
    } else if (align == '^') {
        before = missing / 2;
    }
    for (int64_t i = 0; i < before; i++) {
        memcpy(out + at, fill, (size_t)fill_len);
        at += fill_len;
    }
    memcpy(out + at, sign, (size_t)sign_len);
    at += sign_len;
    if (zero) {
        for (int64_t i = 0; i < missing; i++) {
            out[at++] = '0';
        }
    }
    memcpy(out + at, body, (size_t)body_len);
    at += body_len;
    int64_t after = zero ? 0 : missing - before;
    for (int64_t i = 0; i < after; i++) {
        memcpy(out + at, fill, (size_t)fill_len);
        at += fill_len;
    }
    KrString *result = kr_string_new(out, at);
    kr_free(out);
    return result;
}

static KrString *format_double(double value, const Spec *spec) {
    char body[4200];
    double magnitude = fabs(value);
    bool negative = signbit(value) && !isnan(value);
    if (spec->kind == 'e' || spec->kind == 'E') {
        if (spec->precision >= 0) {
            kr_c_snprintf(body, sizeof(body), spec->kind == 'e' ? "%.*e" : "%.*E",
                     (int)spec->precision, magnitude);
        } else {
            kr_c_snprintf(body, sizeof(body), spec->kind == 'e' ? "%e" : "%E", magnitude);
        }
    } else if (spec->precision >= 0) {
        kr_c_snprintf(body, sizeof(body), "%.*f", (int)spec->precision, magnitude);
    } else {
        KrString *plain = kr_string_from_double(magnitude);
        int64_t limit = (int64_t)sizeof(body) - 1;
        int64_t len = plain->len < limit ? plain->len : limit;
        memcpy(body, plain->data, (size_t)len);
        body[len] = 0;
        kr_string_release(plain);
    }
    const char *sign = negative ? "-" : (spec->plus ? "+" : "");
    return pad(spec, sign, body, (int64_t)strlen(body), '>', isfinite(value));
}

KrString *kr_format_long(int64_t value, const KrString *text) {
    Spec spec;
    if (!parse_spec(text, &spec) || (spec.alternate && spec.kind == 0)) {
        return NULL;
    }
    if (spec.kind == 'e' || spec.kind == 'E' || spec.precision >= 0) {
        return format_double((double)value, &spec);
    }
    uint64_t magnitude = value < 0 ? (uint64_t)0 - (uint64_t)value : (uint64_t)value;
    char digits[80];
    int64_t len = 0;
    unsigned base = 10;
    const char *alphabet = "0123456789abcdef";
    const char *prefix = "";
    switch (spec.kind) {
    case 'x':
        base = 16;
        prefix = "0x";
        break;
    case 'X':
        base = 16;
        alphabet = "0123456789ABCDEF";
        prefix = "0x";
        break;
    case 'b':
        base = 2;
        prefix = "0b";
        break;
    case 'o':
        base = 8;
        prefix = "0o";
        break;
    default:
        break;
    }
    do {
        digits[len++] = alphabet[magnitude % base];
        magnitude /= base;
    } while (magnitude != 0);
    char body[90];
    int64_t at = 0;
    for (int64_t i = len - 1; i >= 0; i--) {
        body[at++] = digits[i];
    }
    char sign[8];
    snprintf(sign, sizeof(sign), "%s%s", value < 0 ? "-" : (spec.plus ? "+" : ""),
             spec.alternate ? prefix : "");
    return pad(&spec, sign, body, at, '>', true);
}

KrString *kr_format_double(double value, const KrString *text) {
    Spec spec;
    if (!parse_spec(text, &spec) || spec.alternate || spec.kind == 'x' || spec.kind == 'X' ||
        spec.kind == 'b' || spec.kind == 'o') {
        return NULL;
    }
    return format_double(value, &spec);
}

KrString *kr_format_text(const KrString *value, const KrString *text) {
    Spec spec;
    if (!parse_spec(text, &spec) || spec.plus || spec.alternate || spec.zero || spec.kind != 0) {
        return NULL;
    }
    int64_t len = value->len;
    if (spec.precision >= 0) {
        int64_t chars = 0;
        int64_t at = 0;
        while (at < value->len && chars < spec.precision) {
            at += utf8_char_len((unsigned char)value->data[at]);
            chars++;
        }
        len = at < value->len ? at : value->len;
    }
    return pad(&spec, "", value->data, len, '<', false);
}
