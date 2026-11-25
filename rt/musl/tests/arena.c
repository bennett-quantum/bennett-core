/* A 16 KiB arena: four one-page mappings fit, the fifth is the fault out_of_memory, not MAP_FAILED. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/mman.h>
int main(void) {
    for (int pages = 1;; ++pages) {
        if (mmap(0, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == MAP_FAILED) {
            printf("MAP_FAILED\n");
            return 1;
        }
        printf("page %d\n", pages);
        fflush(stdout);
    }
}
