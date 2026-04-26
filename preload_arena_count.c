#define _GNU_SOURCE
#include <dlfcn.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>

static void* (*real_malloc)(size_t) = NULL;
static void  (*real_free)(void*) = NULL;
static void* (*real_calloc)(size_t, size_t) = NULL;
static void* (*real_realloc)(void*, size_t) = NULL;

static char bootstrap_buf[131072];
static size_t bootstrap_pos = 0;

#define SLAB_SIZE    ((size_t)4 * 1024 * 1024 * 1024)
#define MAX_THREADS  16
#define REGION_SIZE  (SLAB_SIZE * MAX_THREADS)

static volatile uintptr_t region_base = 0;
static volatile int region_init_done = 0;
static volatile int thread_idx_counter = 0;
static volatile int global_generation = 0;
static volatile int warmup_done = 0;
static volatile int alloc_active = 0;

static __thread uintptr_t arena_ptr = 0;
static __thread uintptr_t arena_end = 0;
static __thread uintptr_t arena_base = 0;
static __thread int arena_gen = -1;

static __thread long cnt_arena = 0;
static __thread long cnt_fallback = 0;
static __thread long cnt_free_arena = 0;
static __thread long cnt_free_system = 0;
static __thread int reported = 0;

static volatile long g_arena = 0;
static volatile long g_fallback = 0;
static volatile long g_free_arena = 0;
static volatile long g_free_system = 0;

static void report_thread(void) {
    if (reported) return;
    reported = 1;
    __sync_fetch_and_add(&g_arena, cnt_arena);
    __sync_fetch_and_add(&g_fallback, cnt_fallback);
    __sync_fetch_and_add(&g_free_arena, cnt_free_arena);
    __sync_fetch_and_add(&g_free_system, cnt_free_system);
}

static void report_all(void) {
    report_thread();
    fprintf(stderr, "\n=== zk-alloc counters ===\n");
    fprintf(stderr, "malloc arena:   %ld\n", g_arena);
    fprintf(stderr, "malloc fallback:%ld\n", g_fallback);
    fprintf(stderr, "free arena:     %ld\n", g_free_arena);
    fprintf(stderr, "free system:    %ld\n", g_free_system);
    fprintf(stderr, "arena pct:      %.1f%%\n",
            100.0 * g_arena / (g_arena + g_fallback + 1));
    fprintf(stderr, "=========================\n");
}

static void ensure_region(void) {
    if (__builtin_expect(region_init_done == 1, 1)) return;
    if (__sync_bool_compare_and_swap(&region_init_done, 0, 2)) {
        void* p = mmap(NULL, REGION_SIZE,
                       PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) { region_init_done = 1; return; }
        madvise(p, REGION_SIZE, MADV_HUGEPAGE);
        region_base = (uintptr_t)p;
        __sync_synchronize();
        region_init_done = 1;
        atexit(report_all);
    }
    while (region_init_done != 1) {}
}

static void init_real(void) {
    real_malloc  = dlsym(RTLD_NEXT, "malloc");
    real_free    = dlsym(RTLD_NEXT, "free");
    real_calloc  = dlsym(RTLD_NEXT, "calloc");
    real_realloc = dlsym(RTLD_NEXT, "realloc");
}

void zk_alloc_phase_boundary(void) {
    if (warmup_done == 0) { warmup_done = 1; ensure_region(); return; }
    __sync_fetch_and_add(&global_generation, 1);
    __sync_synchronize();
    alloc_active = 1;
}

void zk_alloc_deactivate(void) {
    alloc_active = 0;
}

static inline void __attribute__((always_inline)) check_reset(void) {
    int gen = global_generation;
    if (__builtin_expect(arena_gen != gen, 0)) {
        if (arena_base == 0) {
            int idx = __sync_fetch_and_add(&thread_idx_counter, 1);
            if (idx >= MAX_THREADS) return;
            arena_base = region_base + (uintptr_t)idx * SLAB_SIZE;
            arena_end = arena_base + SLAB_SIZE;
        }
        arena_ptr = arena_base;
        arena_gen = gen;
        report_thread();
        reported = 0;
        cnt_arena = 0;
        cnt_fallback = 0;
        cnt_free_arena = 0;
        cnt_free_system = 0;
    }
}

void* malloc(size_t size) {
    if (__builtin_expect(!real_malloc, 0)) {
        init_real();
        if (!real_malloc) {
            size_t al = (bootstrap_pos + 15) & ~(size_t)15;
            if (al + size <= sizeof(bootstrap_buf)) {
                void* p = &bootstrap_buf[al];
                bootstrap_pos = al + size;
                return p;
            }
            return NULL;
        }
    }
    if (__builtin_expect(alloc_active, 1)) {
        check_reset();
        if (__builtin_expect(arena_end != 0, 1)) {
            uintptr_t p = arena_ptr;
            uintptr_t al = (p + 15) & ~(uintptr_t)15;
            uintptr_t np = al + size;
            if (__builtin_expect(np <= arena_end, 1)) {
                arena_ptr = np;
                cnt_arena++;
                return (void*)al;
            }
        }
    }
    cnt_fallback++;
    return real_malloc(size);
}

void free(void* ptr) {
    if (!ptr) return;
    if ((char*)ptr >= bootstrap_buf && (char*)ptr < bootstrap_buf + sizeof(bootstrap_buf))
        return;
    if (region_base && (uintptr_t)ptr >= region_base && (uintptr_t)ptr < region_base + REGION_SIZE) {
        cnt_free_arena++;
        return;
    }
    cnt_free_system++;
    if (real_free) real_free(ptr);
}

void* calloc(size_t nmemb, size_t size) {
    if (__builtin_expect(!real_calloc, 0)) {
        size_t total = nmemb * size;
        void* p = malloc(total);
        if (p) memset(p, 0, total);
        return p;
    }
    if (__builtin_expect(alloc_active, 1)) {
        size_t total = nmemb * size;
        check_reset();
        if (__builtin_expect(arena_end != 0, 1)) {
            uintptr_t p = arena_ptr;
            uintptr_t al = (p + 15) & ~(uintptr_t)15;
            uintptr_t np = al + total;
            if (__builtin_expect(np <= arena_end, 1)) {
                arena_ptr = np;
                cnt_arena++;
                memset((void*)al, 0, total);
                return (void*)al;
            }
        }
    }
    cnt_fallback++;
    return real_calloc(nmemb, size);
}

void* realloc(void* ptr, size_t size) {
    if (__builtin_expect(!real_realloc, 0)) { init_real(); if (!real_realloc) return NULL; }
    if (!ptr) return malloc(size);
    if ((char*)ptr >= bootstrap_buf && (char*)ptr < bootstrap_buf + sizeof(bootstrap_buf)) {
        void* p = malloc(size);
        if (p) memcpy(p, ptr, size);
        return p;
    }
    if (region_base && (uintptr_t)ptr >= region_base && (uintptr_t)ptr < region_base + REGION_SIZE) {
        void* p = malloc(size);
        if (p) memcpy(p, ptr, size);
        return p;
    }
    return real_realloc(ptr, size);
}

static void* (*real_memalign)(size_t, size_t) = NULL;

void* memalign(size_t alignment, size_t size) {
    if (!real_memalign) real_memalign = dlsym(RTLD_NEXT, "memalign");
    if (__builtin_expect(alloc_active, 1)) {
        check_reset();
        if (__builtin_expect(arena_end != 0, 1)) {
            uintptr_t p = arena_ptr;
            uintptr_t al = (p + alignment - 1) & ~(uintptr_t)(alignment - 1);
            uintptr_t np = al + size;
            if (__builtin_expect(np <= arena_end, 1)) {
                arena_ptr = np;
                cnt_arena++;
                return (void*)al;
            }
        }
    }
    cnt_fallback++;
    return real_memalign(alignment, size);
}
int posix_memalign(void** memptr, size_t alignment, size_t size) {
    void* p = memalign(alignment, size);
    if (!p) return 12;
    *memptr = p;
    return 0;
}
void* aligned_alloc(size_t alignment, size_t size) { return memalign(alignment, size); }
