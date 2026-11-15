#define _GNU_SOURCE
#include <signal.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <unistd.h>
#ifndef CASE
#define CASE 0
#endif
int main(void) {
#if CASE == 0
    write(2, "", 0);
#elif CASE == 1
    struct iovec v = {"X", 1}; writev(2, &v, 1);
#elif CASE == 2
    syscall(SYS_open, "anything", 0, 0, 0, 0, 0);
#elif CASE == 3
    syscall(SYS_openat, -100, "anything", 0, 0, 0, 0);
#elif CASE == 4
    syscall(999999, 0, 0, 0, 0, 0, 0);
#elif CASE == 5
    mmap(0, 4096, 3, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
#elif CASE == 6
    mmap(0, 4096, 3, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
#elif CASE == 7
    mmap(0, 4096, 3, MAP_PRIVATE, 0, 0);
#elif CASE == 8
    void *p = mmap(0, 4096, 3, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    mprotect(p, 4096, PROT_READ);
#elif CASE == 9
    void *p = mmap(0, 4096, 3, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    madvise(p, 4096, MADV_NORMAL);
#elif CASE == 10
    unsigned long act[4] = {0};
    syscall(SYS_rt_sigaction, SIGUSR1, act, 0, 8, 0, 0);
#endif
    return 99;
}
