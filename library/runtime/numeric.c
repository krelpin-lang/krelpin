/* Number conversions that ignore the locale.
 *
 * `strtod` and `snprintf` follow LC_NUMERIC, so a host program that calls
 * `setlocale` could make `1.5` print as `1,5` or fail to parse. Krelpin
 * text is the same everywhere, so these switch the calling thread to the
 * "C" locale for the duration of the call. `uselocale` only affects the
 * current thread, which keeps this safe while other threads run. */

#define _GNU_SOURCE

#include <locale.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#if defined(__APPLE__)
#include <xlocale.h>
#endif

#include "krelpin.h"

static locale_t c_locale;
static pthread_once_t c_locale_once = PTHREAD_ONCE_INIT;

static void make_c_locale(void) {
    c_locale = newlocale(LC_ALL_MASK, "C", (locale_t)0);
}

/* Returns the locale to restore, or 0 when nothing was changed. */
static locale_t enter_c_locale(void) {
    pthread_once(&c_locale_once, make_c_locale);
    if (c_locale == (locale_t)0) {
        return (locale_t)0;
    }
    return uselocale(c_locale);
}

static void leave_c_locale(locale_t previous) {
    if (previous != (locale_t)0) {
        uselocale(previous);
    }
}

double kr_c_strtod(const char *text, char **end) {
    locale_t previous = enter_c_locale();
    double value = strtod(text, end);
    leave_c_locale(previous);
    return value;
}

int kr_c_snprintf(char *buffer, size_t size, const char *format, ...) {
    va_list args;
    va_start(args, format);
    locale_t previous = enter_c_locale();
    int len = vsnprintf(buffer, size, format, args);
    leave_c_locale(previous);
    va_end(args);
    return len;
}
