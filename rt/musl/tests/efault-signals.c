/* Structure-pointer and error-order checks for signal syscalls:
 * rt_sigaction, rt_sigprocmask, and rt_sigpending.
 *
 * argv[1] selects a case (or "agree" for the batch of non-faulting agreeing cases).
 * Optional argv[2] supplies the end-of-memory address on the device for partial-object cases;
 * natively, partial_edge maps a read/write page followed by a PROT_NONE guard page. */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

struct ksa {
    void (*handler)(int);
    unsigned long flags;
    void (*restorer)(void);
    unsigned long mask;
};

static char *partial_edge(int argc, char **argv)
{
    if (argc >= 3)
        return (char *)(uintptr_t)strtoull(argv[2], NULL, 10);
    char *pages = mmap(NULL, 8192, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (pages == MAP_FAILED) _exit(99);
    if (mprotect(pages + 4096, 4096, PROT_NONE) != 0) _exit(98);
    return pages + 4096;
}

static void report(const char *label, long ret, int err)
{
    printf("%s: ret=%ld errno=%d\n", label, ret, err);
}

int main(int argc, char **argv)
{
    if (argc < 2) return 2;
    const char *c = argv[1];
    struct ksa act = { SIG_DFL, 0, NULL, 0 }, old = { 0 };
    unsigned long set = 0, oldset = 0;
    void *low = (void *)(uintptr_t)8;
    void *wrap8 = (void *)(uintptr_t)-8;
    void *wrap4 = (void *)(uintptr_t)-4;
    long r;

    if (!strcmp(c, "agree")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, &act, &old, 8);
        report("sigaction-valid", r, errno);
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, &act, NULL, 4);
        report("sigaction-badsize", r, errno);
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, edge - 8, NULL, 4);
        report("sigaction-badsize-partial8", r, errno);
        errno = 0; r = syscall(SYS_rt_sigaction, SIGKILL, &act, NULL, 8);
        report("sigaction-sigkill-valid", r, errno);
        errno = 0; r = syscall(SYS_rt_sigaction, 0, NULL, edge - 8, 8);
        report("sigaction-sig0-old-partial8", r, errno);

        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, &set, &oldset, 8);
        report("sigprocmask-valid", r, errno);
        errno = 0; r = syscall(SYS_rt_sigprocmask, 99, &set, NULL, 8);
        report("sigprocmask-badhow-valid", r, errno);
        errno = 0; r = syscall(SYS_rt_sigprocmask, 99, NULL, &oldset, 8);
        report("sigprocmask-badhow-null", r, errno);
        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, edge - 4, NULL, 4);
        report("sigprocmask-badsize-partial4", r, errno);
        errno = 0; r = syscall(SYS_rt_sigprocmask, 99, &set, edge - 4, 8);
        report("sigprocmask-badhow-old-partial4", r, errno);

        errno = 0; r = syscall(SYS_rt_sigpending, &oldset, 8);
        report("sigpending-valid", r, errno);
        errno = 0; r = syscall(SYS_rt_sigpending, &oldset, 16);
        report("sigpending-size16", r, errno);
        errno = 0; r = syscall(SYS_rt_sigpending, NULL, 16);
        report("sigpending-size16-null", r, errno);
        return 0;
    }

    if (!strcmp(c, "sigaction-alias")) {
        struct ksa sa = { SIG_IGN, 0, NULL, 0 }, check = { 0 };
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, &sa, &sa, 8);
        int e = errno;
        syscall(SYS_rt_sigaction, SIGUSR1, NULL, &check, 8);
        printf("%s: ret=%ld errno=%d installed_ign=%d old_dfl=%d\n",
               c, r, e, check.handler == SIG_IGN, sa.handler == SIG_DFL);
        return 0;
    }
    if (!strcmp(c, "sigaction-sigkill-low8")) {
        errno = 0; r = syscall(SYS_rt_sigaction, SIGKILL, low, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-sigkill-wrap8")) {
        errno = 0; r = syscall(SYS_rt_sigaction, SIGKILL, wrap8, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-sigkill-partial8")) {
        char *edge = partial_edge(argc, argv);
        *(unsigned long *)(edge - 8) = 0;
        errno = 0; r = syscall(SYS_rt_sigaction, SIGKILL, edge - 8, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-sigstop-partial8")) {
        char *edge = partial_edge(argc, argv);
        *(unsigned long *)(edge - 8) = 0;
        errno = 0; r = syscall(SYS_rt_sigaction, SIGSTOP, edge - 8, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-sig0-partial8")) {
        char *edge = partial_edge(argc, argv);
        *(unsigned long *)(edge - 8) = 0;
        errno = 0; r = syscall(SYS_rt_sigaction, 0, edge - 8, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-sig65-partial8")) {
        char *edge = partial_edge(argc, argv);
        *(unsigned long *)(edge - 8) = 0;
        errno = 0; r = syscall(SYS_rt_sigaction, 65, edge - 8, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-act-low8")) {
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, low, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-act-wrap8")) {
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, wrap8, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-act-partial8")) {
        char *edge = partial_edge(argc, argv);
        *(unsigned long *)(edge - 8) = 0;
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, edge - 8, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-act-partial24")) {
        char *edge = partial_edge(argc, argv);
        memset(edge - 24, 0, 24);
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, edge - 24, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-old-low8")) {
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, NULL, low, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-old-wrap8")) {
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, NULL, wrap8, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigaction-old-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_rt_sigaction, SIGUSR1, NULL, edge - 8, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-badhow-low8")) {
        errno = 0; r = syscall(SYS_rt_sigprocmask, 99, low, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-badhow-wrap4")) {
        errno = 0; r = syscall(SYS_rt_sigprocmask, 99, wrap4, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-badhow-partial4")) {
        char *edge = partial_edge(argc, argv);
        *(unsigned int *)(edge - 4) = 0;
        errno = 0; r = syscall(SYS_rt_sigprocmask, 99, edge - 4, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-set-low8")) {
        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, low, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-set-wrap4")) {
        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, wrap4, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-set-partial4")) {
        char *edge = partial_edge(argc, argv);
        *(unsigned int *)(edge - 4) = 0;
        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, edge - 4, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-old-low8")) {
        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, NULL, low, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-old-wrap4")) {
        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, NULL, wrap4, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigprocmask-old-partial4")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_rt_sigprocmask, SIG_BLOCK, NULL, edge - 4, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigpending-size4")) {
        errno = 0; r = syscall(SYS_rt_sigpending, &oldset, 4);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigpending-null")) {
        errno = 0; r = syscall(SYS_rt_sigpending, NULL, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigpending-low8")) {
        errno = 0; r = syscall(SYS_rt_sigpending, low, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigpending-wrap4")) {
        errno = 0; r = syscall(SYS_rt_sigpending, wrap4, 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "sigpending-partial4")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_rt_sigpending, edge - 4, 8);
        report(c, r, errno); return 0;
    }

    return 3;
}
