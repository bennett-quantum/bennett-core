/* A program's own anonymous mapping larger than the free arena is the fault out_of_memory. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/mman.h>
int main(void) {
    char *small = mmap(0, 8192, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    printf("small %d\n", small != MAP_FAILED);
    fflush(stdout);
    char *large = mmap(0, 2 << 20, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    printf("large %d\n", large != MAP_FAILED);
    return 1;
}
