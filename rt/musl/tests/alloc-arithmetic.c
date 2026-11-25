/* Requests the C library rejects by arithmetic alone stay null results with ENOMEM, as natively: they do
 * not depend on the arena, so they are not exhaustion. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
static void report(const char *what, int failed) {
    printf("%s %d %d\n", what, failed, errno == ENOMEM);
    errno = 0;
}
int main(void) {
    char *p = malloc(16);
    p[0] = 5;
    report("malloc", malloc(SIZE_MAX) == 0);
    report("malloc-half", malloc(SIZE_MAX / 2) == 0);
    report("calloc", calloc(SIZE_MAX / 2, 4) == 0);
    report("aligned_alloc", aligned_alloc(16, SIZE_MAX - 8) == 0);
    report("realloc", realloc(p, SIZE_MAX) == 0);
    report("reallocarray", reallocarray(p, SIZE_MAX / 2, 4) == 0);
    report("mmap", mmap(0, PTRDIFF_MAX, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == MAP_FAILED);
    printf("kept %d\n", p[0]);
    free(p);
    return 0;
}
