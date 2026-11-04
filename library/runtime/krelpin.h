#ifndef KRELPIN_H
#define KRELPIN_H

#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct KrType KrType;

struct KrType {
    const char *name;
    const KrType *super;
    void (*drop)(void *object);
    int64_t size;
    const KrType *const *interfaces;
    int64_t kind;
    int64_t vtable_len;
    void *vtable[];
};

enum {
    KR_KIND_CLASS = 0,
    KR_KIND_STRING = 1,
    KR_KIND_ARRAY = 2,
    KR_KIND_BOX = 3,
    KR_KIND_CLOSURE = 4,
    KR_KIND_INTERFACE = 5,
};

typedef struct {
    const KrType *type;
} KrObject;

typedef struct {
    const KrType *type;
    _Atomic int64_t refs;
    int64_t len;
    int64_t chars;
    char data[];
} KrString;

typedef struct {
    const KrType *type;
    int64_t len;
    _Alignas(16) unsigned char data[];
} KrArray;

typedef struct {
    int32_t (*hash)(const void *key);
    bool (*eq)(const void *a, const void *b);
    void (*drop_key)(void *key);
    void (*drop_value)(void *value);
    int64_t entry_size;
    int64_t key_offset;
    int64_t value_offset;
    int64_t value_size;
} KrTableShape;

typedef struct {
    unsigned char *entries;
    int64_t entry_count;
    int64_t entry_cap;
    int64_t live;
    int64_t *index;
    int64_t index_cap;
} KrTable;

extern const KrType kr_string_type;
extern _Thread_local void *kr_pending;

void *kr_alloc(int64_t size);
void *kr_realloc(void *pointer, int64_t size);
void kr_free(void *pointer);
void *kr_new_object(const KrType *type);
void kr_drop_object(void *object);
bool kr_instance_of(const void *object, const KrType *target);
_Noreturn void kr_panic(const char *message);

KrString *kr_string_new(const char *bytes, int64_t len);
void kr_string_retain(KrString *string);
void kr_string_release(void *string);
KrString *kr_string_concat(const KrString *a, const KrString *b);
int32_t kr_string_length(const KrString *string);
int64_t kr_string_byte_offset(const KrString *string, int64_t chars);
KrArray *kr_string_chars(const KrString *string, const KrType *array_type);
KrString *kr_string_substring(const KrString *string, int32_t begin, int32_t end);
int32_t kr_string_index_of(const KrString *string, const KrString *text);
bool kr_string_starts_with(const KrString *string, const KrString *prefix);
bool kr_string_ends_with(const KrString *string, const KrString *suffix);
KrString *kr_string_to_upper(const KrString *string);
KrString *kr_string_to_lower(const KrString *string);
KrString *kr_string_trim(const KrString *string);
KrString *kr_string_replace(const KrString *string, const KrString *target,
                            const KrString *replacement);
KrString *kr_string_repeat(const KrString *string, int32_t count);
KrArray *kr_string_split(const KrString *string, const KrString *separator,
                         const KrType *array_type);
int32_t kr_string_compare(const KrString *a, const KrString *b);
bool kr_string_equals(const KrString *a, const KrString *b);
int32_t kr_string_hash(const KrString *string);
KrString *kr_string_join(const KrString *separator, const KrArray *parts);
KrString *kr_string_from_long(int64_t value);
KrString *kr_string_from_double(double value);
KrString *kr_string_from_float(float value);
KrString *kr_string_from_char(int32_t value);
KrString *kr_string_from_bool(bool value);
KrString *kr_string_describe(const void *object);
KrString *kr_string_bounds(int64_t index, int64_t len);
KrString *kr_string_cast(const void *object, const KrString *target);
KrString *kr_string_number_format(const KrString *text);
bool kr_parse_long(const KrString *text, int64_t min, int64_t max, int64_t *out);
bool kr_parse_double(const KrString *text, double *out);

void kr_console_print(const KrString *string);
void kr_console_newline(void);
KrString *kr_console_read_line(void);
void kr_uncaught(const KrType *type, const KrString *message);
void kr_report_thread(const KrType *type, const KrString *message);
void kr_test_begin(int64_t count);
void kr_test_passed(const char *name);
void kr_test_failed(const char *name, const KrType *type, const KrString *message);
int32_t kr_test_end(void);
void kr_export_failed(const KrType *type, const KrString *message);
void kr_once(_Atomic int32_t *state, void (*init)(void));

bool kr_char_is_digit(int32_t c);
bool kr_char_is_letter(int32_t c);
bool kr_char_is_whitespace(int32_t c);
bool kr_char_is_upper(int32_t c);
bool kr_char_is_lower(int32_t c);
int32_t kr_char_to_upper(int32_t c);
int32_t kr_char_to_lower(int32_t c);

double kr_random(void);

void *kr_thread_spawn(void (*entry)(void *), void *argument);
void kr_thread_join(void *handle);
void kr_thread_sleep(int64_t millis);
void *kr_mutex_new(void);
void kr_mutex_lock(void *mutex);
void kr_mutex_unlock(void *mutex);
void kr_mutex_free(void *mutex);

void kr_table_init(KrTable *table);
void kr_table_free(KrTable *table, const KrTableShape *shape);
void kr_table_clear(KrTable *table, const KrTableShape *shape);
void *kr_table_find(KrTable *table, const KrTableShape *shape, const void *key);
void *kr_table_insert(KrTable *table, const KrTableShape *shape, void *entry, bool *existed);
bool kr_table_remove(KrTable *table, const KrTableShape *shape, const void *key, void *out);
int64_t kr_table_slot_count(const KrTable *table);
void *kr_table_slot(const KrTable *table, const KrTableShape *shape, int64_t index);

void kr_runtime_init(int argc, char **argv);
void kr_runtime_exit(void);
KrArray *kr_runtime_args(const KrType *array_type);

#endif
