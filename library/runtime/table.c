#include <string.h>

#include "krelpin.h"

enum {
    EMPTY = -1,
    REMOVED = -2,
};

typedef struct {
    int32_t hash;
    int32_t alive;
} EntryHeader;

static EntryHeader *entry_at(const KrTable *table, const KrTableShape *shape, int64_t index) {
    return (EntryHeader *)(table->entries + index * shape->entry_size);
}

static void *key_of(EntryHeader *entry, const KrTableShape *shape) {
    return (unsigned char *)entry + shape->key_offset;
}

static void *value_of(EntryHeader *entry, const KrTableShape *shape) {
    return (unsigned char *)entry + shape->value_offset;
}

void kr_table_init(KrTable *table) {
    memset(table, 0, sizeof *table);
}

static void rebuild_index(KrTable *table, const KrTableShape *shape, int64_t cap) {
    kr_free(table->index);
    table->index = kr_alloc(cap * (int64_t)sizeof(int64_t));
    table->index_cap = cap;
    for (int64_t i = 0; i < cap; i++) {
        table->index[i] = EMPTY;
    }
    for (int64_t i = 0; i < table->entry_count; i++) {
        EntryHeader *entry = entry_at(table, shape, i);
        if (!entry->alive) {
            continue;
        }
        uint64_t slot = (uint64_t)(uint32_t)entry->hash & (uint64_t)(cap - 1);
        while (table->index[slot] != EMPTY) {
            slot = (slot + 1) & (uint64_t)(cap - 1);
        }
        table->index[slot] = i;
    }
}

static void compact(KrTable *table, const KrTableShape *shape) {
    int64_t kept = 0;
    for (int64_t i = 0; i < table->entry_count; i++) {
        EntryHeader *entry = entry_at(table, shape, i);
        if (!entry->alive) {
            continue;
        }
        if (kept != i) {
            memcpy(entry_at(table, shape, kept), entry, (size_t)shape->entry_size);
        }
        kept++;
    }
    table->entry_count = kept;
}

static void reserve(KrTable *table, const KrTableShape *shape) {
    if (table->entry_count == table->entry_cap) {
        if (table->live * 2 < table->entry_count) {
            compact(table, shape);
            rebuild_index(table, shape, table->index_cap);
        } else {
            int64_t cap = table->entry_cap == 0 ? 8 : table->entry_cap * 2;
            table->entries = kr_realloc(table->entries, cap * shape->entry_size);
            table->entry_cap = cap;
        }
    }
    if ((table->entry_count + 1) * 4 > table->index_cap * 3) {
        int64_t cap = table->index_cap == 0 ? 16 : table->index_cap * 2;
        while ((table->entry_count + 1) * 4 > cap * 3) {
            cap *= 2;
        }
        rebuild_index(table, shape, cap);
    }
}

static int64_t locate(const KrTable *table, const KrTableShape *shape, const void *key,
                      int32_t hash) {
    if (table->index_cap == 0) {
        return -1;
    }
    uint64_t mask = (uint64_t)(table->index_cap - 1);
    for (uint64_t slot = (uint64_t)(uint32_t)hash & mask;; slot = (slot + 1) & mask) {
        int64_t index = table->index[slot];
        if (index == EMPTY) {
            return -1;
        }
        if (index == REMOVED) {
            continue;
        }
        EntryHeader *entry = entry_at(table, shape, index);
        if (entry->hash == hash && shape->eq(key_of(entry, shape), key)) {
            return (int64_t)slot;
        }
    }
}

void *kr_table_find(KrTable *table, const KrTableShape *shape, const void *key) {
    int64_t slot = locate(table, shape, key, shape->hash(key));
    if (slot < 0 || kr_pending != NULL) {
        return NULL;
    }
    return entry_at(table, shape, table->index[slot]);
}

void *kr_table_insert(KrTable *table, const KrTableShape *shape, void *entry, bool *existed) {
    EntryHeader *incoming = entry;
    int32_t hash = shape->hash(key_of(incoming, shape));
    int64_t slot = locate(table, shape, key_of(incoming, shape), hash);
    if (kr_pending != NULL) {
        *existed = false;
        return NULL;
    }
    if (slot >= 0) {
        EntryHeader *stored = entry_at(table, shape, table->index[slot]);
        unsigned char swap[shape->value_size > 0 ? shape->value_size : 1];
        memcpy(swap, value_of(stored, shape), (size_t)shape->value_size);
        memcpy(value_of(stored, shape), value_of(incoming, shape), (size_t)shape->value_size);
        memcpy(value_of(incoming, shape), swap, (size_t)shape->value_size);
        *existed = true;
        return stored;
    }
    reserve(table, shape);
    int64_t index = table->entry_count++;
    EntryHeader *stored = entry_at(table, shape, index);
    memcpy(stored, incoming, (size_t)shape->entry_size);
    stored->hash = hash;
    stored->alive = 1;
    uint64_t mask = (uint64_t)(table->index_cap - 1);
    uint64_t at = (uint64_t)(uint32_t)hash & mask;
    while (table->index[at] >= 0) {
        at = (at + 1) & mask;
    }
    table->index[at] = index;
    table->live++;
    *existed = false;
    return stored;
}

bool kr_table_remove(KrTable *table, const KrTableShape *shape, const void *key, void *out) {
    int64_t slot = locate(table, shape, key, shape->hash(key));
    if (slot < 0 || kr_pending != NULL) {
        return false;
    }
    EntryHeader *entry = entry_at(table, shape, table->index[slot]);
    memcpy(out, entry, (size_t)shape->entry_size);
    entry->alive = 0;
    table->index[slot] = REMOVED;
    table->live--;
    return true;
}

void kr_table_clear(KrTable *table, const KrTableShape *shape) {
    for (int64_t i = 0; i < table->entry_count; i++) {
        EntryHeader *entry = entry_at(table, shape, i);
        if (!entry->alive) {
            continue;
        }
        entry->alive = 0;
        if (shape->drop_key != NULL) {
            shape->drop_key(key_of(entry, shape));
        }
        if (shape->drop_value != NULL) {
            shape->drop_value(value_of(entry, shape));
        }
    }
    table->entry_count = 0;
    table->live = 0;
    for (int64_t i = 0; i < table->index_cap; i++) {
        table->index[i] = EMPTY;
    }
}

void kr_table_free(KrTable *table, const KrTableShape *shape) {
    kr_table_clear(table, shape);
    kr_free(table->entries);
    kr_free(table->index);
    kr_table_init(table);
}

int64_t kr_table_slot_count(const KrTable *table) {
    return table->entry_count;
}

void *kr_table_slot(const KrTable *table, const KrTableShape *shape, int64_t index) {
    if (index < 0 || index >= table->entry_count) {
        return NULL;
    }
    EntryHeader *entry = entry_at(table, shape, index);
    return entry->alive ? entry : NULL;
}
