#define _GNU_SOURCE
#include <signal.h>
#include <sys/syscall.h>
#include <unistd.h>
#ifndef CALL
#define CALL SYS_kill
#endif
int main(void) {
#if CALL == SYS_tgkill
    syscall(CALL, 1L, 1L, (long)SIGABRT, 0L, 0L, 0L);
#else
    syscall(CALL, 1L, (long)SIGABRT, 0L, 0L, 0L, 0L);
#endif
    return 99;
}
