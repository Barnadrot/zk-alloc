#define _GNU_SOURCE
#include <dlfcn.h>
#include <stddef.h>
#include <string.h>

static void* (*real_malloc)(size_t) = NULL;
static void  (*real_free)(void*) = NULL;
static void* (*real_calloc)(size_t, size_t) = NULL;
static void* (*real_realloc)(void*, size_t) = NULL;

static char bootstrap_buf[65536];
static size_t bootstrap_pos = 0;

static void init(void) {
    real_malloc  = dlsym(RTLD_NEXT, "malloc");
    real_free    = dlsym(RTLD_NEXT, "free");
    real_calloc  = dlsym(RTLD_NEXT, "calloc");
    real_realloc = dlsym(RTLD_NEXT, "realloc");
}

void* malloc(size_t size) {
    if (__builtin_expect(!real_malloc, 0)) {
        init();
        if (!real_malloc) {
            size_t aligned = (bootstrap_pos + 15) & ~15;
            if (aligned + size <= sizeof(bootstrap_buf)) {
                void* p = &bootstrap_buf[aligned];
                bootstrap_pos = aligned + size;
                return p;
            }
            return NULL;
        }
    }
    return real_malloc(size);
}

void free(void* ptr) {
    if ((char*)ptr >= bootstrap_buf && (char*)ptr < bootstrap_buf + sizeof(bootstrap_buf))
        return;
    if (real_free) real_free(ptr);
}

void* calloc(size_t nmemb, size_t size) {
    if (__builtin_expect(!real_calloc, 0)) {
        size_t total = nmemb * size;
        void* p = malloc(total);
        if (p) memset(p, 0, total);
        return p;
    }
    return real_calloc(nmemb, size);
}

void* realloc(void* ptr, size_t size) {
    if (__builtin_expect(!real_realloc, 0)) {
        init();
        if (!real_realloc) return NULL;
    }
    if ((char*)ptr >= bootstrap_buf && (char*)ptr < bootstrap_buf + sizeof(bootstrap_buf)) {
        void* p = real_malloc(size);
        if (p) memcpy(p, ptr, size);
        return p;
    }
    return real_realloc(ptr, size);
}
