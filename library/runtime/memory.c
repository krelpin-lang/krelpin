#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "krelpin.h"

_Thread_local void *kr_pending;

_Noreturn void kr_panic(const char *message) {
    fflush(stdout);
    fprintf(stderr, "fatal runtime error: %s\n", message);
    abort();
}

void *kr_alloc(int64_t size) {
    void *pointer = calloc(1, size > 0 ? (size_t)size : 1);
    if (pointer == NULL) {
        kr_panic("out of memory");
    }
    return pointer;
}

void *kr_realloc(void *pointer, int64_t size) {
    void *grown = realloc(pointer, size > 0 ? (size_t)size : 1);
    if (grown == NULL) {
        kr_panic("out of memory");
    }
    return grown;
}

void kr_free(void *pointer) {
    free(pointer);
}

void *kr_new_object(const KrType *type) {
    KrObject *object = kr_alloc(type->size);
    object->type = type;
    return object;
}

void kr_drop_object(void *object) {
    if (object == NULL) {
        return;
    }
    const KrType *type = ((KrObject *)object)->type;
    type->drop(object);
}

bool kr_instance_of(const void *object, const KrType *target) {
    if (object == NULL) {
        return false;
    }
    const KrType *type = ((const KrObject *)object)->type;
    if (target->kind == KR_KIND_INTERFACE) {
        for (const KrType *const *entry = type->interfaces; entry != NULL && *entry != NULL;
             entry++) {
            if (*entry == target) {
                return true;
            }
        }
        return false;
    }
    for (; type != NULL; type = type->super) {
        if (type == target) {
            return true;
        }
    }
    return false;
}
