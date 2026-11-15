/* The break: pages given back and taken again read as zero. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(void) {
    char *start = (char *)syscall(SYS_brk, 0);
    char *grown = (char *)syscall(SYS_brk, start + 8192);
    start[4096] = 9;
    syscall(SYS_brk, start);
    syscall(SYS_brk, start + 8192);
    printf("%d %d\n", grown == start + 8192, start[4096]);
    return 0;
}
