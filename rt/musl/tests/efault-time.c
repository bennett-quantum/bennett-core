/* Structure-pointer and error-order checks for time syscalls:
 * clock_gettime, clock_getres, gettimeofday, and time.
 *
 * argv[1] selects a case (or "agree" for the batch of non-faulting agreeing cases).
 * Optional argv[2] supplies the end-of-memory address on the device for partial-object cases;
 * natively, partial_edge maps a read/write page followed by a PROT_NONE guard page. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

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

static void report_nonneg(const char *label, long ret, int err)
{
    printf("%s: ok=%d errno=%d\n", label, ret >= 0 ? 1 : 0, err);
}

int main(int argc, char **argv)
{
    if (argc < 2) return 2;
    const char *c = argv[1];
    struct timespec ts = { -1, -1 };
    struct timeval tv = { -1, -1 };
    int tz[2] = { -1, -1 };
    time_t t = -1;
    void *low = (void *)(uintptr_t)8;
    void *wrap8 = (void *)(uintptr_t)-8;
    void *wrap4 = (void *)(uintptr_t)-4;
    long r;

    if (!strcmp(c, "agree")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_clock_gettime, CLOCK_REALTIME, &ts);
        report("clock_gettime-valid", r, errno);
        errno = 0; r = syscall(SYS_clock_gettime, 99, &ts);
        report("clock_gettime-badclk-valid", r, errno);
        errno = 0; r = syscall(SYS_clock_gettime, 99, NULL);
        report("clock_gettime-badclk-null", r, errno);
        errno = 0; r = syscall(SYS_clock_gettime, 99, low);
        report("clock_gettime-badclk-low8", r, errno);
        errno = 0; r = syscall(SYS_clock_gettime, 99, edge - 8);
        report("clock_gettime-badclk-partial8", r, errno);
        errno = 0; r = syscall(SYS_clock_gettime, CLOCK_REALTIME, NULL);
        report("clock_gettime-null", r, errno);

        errno = 0; r = syscall(SYS_clock_getres, CLOCK_REALTIME, &ts);
        report("clock_getres-valid", r, errno);
        errno = 0; r = syscall(SYS_clock_getres, CLOCK_REALTIME, NULL);
        report("clock_getres-valid-null", r, errno);
        errno = 0; r = syscall(SYS_clock_getres, 99, &ts);
        report("clock_getres-badclk-valid", r, errno);
        errno = 0; r = syscall(SYS_clock_getres, 99, edge - 8);
        report("clock_getres-badclk-partial8", r, errno);

        errno = 0; r = syscall(SYS_gettimeofday, &tv, tz);
        report("gettimeofday-valid", r, errno);
        errno = 0; r = syscall(SYS_gettimeofday, NULL, NULL);
        report("gettimeofday-valid-nulls", r, errno);

        errno = 0; r = syscall(SYS_time, &t);
        report_nonneg("time-valid", (r >= 0 && t == r) ? 0 : -1, errno);
        errno = 0; r = syscall(SYS_time, NULL);
        report_nonneg("time-null", r, errno);
        return 0;
    }

    if (!strcmp(c, "clock_gettime-low8")) {
        errno = 0; r = syscall(SYS_clock_gettime, CLOCK_REALTIME, low);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "clock_gettime-wrap8")) {
        errno = 0; r = syscall(SYS_clock_gettime, CLOCK_REALTIME, wrap8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "clock_gettime-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_clock_gettime, CLOCK_REALTIME, edge - 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "clock_getres-low8")) {
        errno = 0; r = syscall(SYS_clock_getres, CLOCK_REALTIME, low);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "clock_getres-wrap8")) {
        errno = 0; r = syscall(SYS_clock_getres, CLOCK_REALTIME, wrap8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "clock_getres-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_clock_getres, CLOCK_REALTIME, edge - 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "gettimeofday-tv-low8")) {
        errno = 0; r = syscall(SYS_gettimeofday, low, NULL);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "gettimeofday-tv-wrap8")) {
        errno = 0; r = syscall(SYS_gettimeofday, wrap8, NULL);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "gettimeofday-tv-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_gettimeofday, edge - 8, NULL);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "gettimeofday-tz-low8")) {
        errno = 0; r = syscall(SYS_gettimeofday, NULL, low);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "gettimeofday-tz-wrap4")) {
        errno = 0; r = syscall(SYS_gettimeofday, NULL, wrap4);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "gettimeofday-tz-partial4")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_gettimeofday, NULL, edge - 4);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "time-low8")) {
        errno = 0; r = syscall(SYS_time, low);
        report_nonneg(c, r, errno); return 0;
    }
    if (!strcmp(c, "time-wrap4")) {
        errno = 0; r = syscall(SYS_time, wrap4);
        report_nonneg(c, r, errno); return 0;
    }
    if (!strcmp(c, "time-partial4")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_time, edge - 4);
        report_nonneg(c, r, errno); return 0;
    }

    return 3;
}
