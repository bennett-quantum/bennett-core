/* A raw break growth the arena cannot hold is the fault out_of_memory, not the old break. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(void) {
    char *start = (char *)syscall(SYS_brk, 0);
    printf("grown %d\n", (char *)syscall(SYS_brk, start + 8192) == start + 8192);
    fflush(stdout);
    char *far = (char *)syscall(SYS_brk, start + (2 << 20));
    printf("far %d\n", far == start + (2 << 20));
    return 1;
}
