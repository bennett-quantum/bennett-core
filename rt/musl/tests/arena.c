#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void) {
    int pages = 0;
    while (mmap(0, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) != MAP_FAILED) ++pages;
    printf("pages=%d errno=%d\n", pages, errno);
    return pages == 4 && errno == ENOMEM ? 0 : 1;
}
