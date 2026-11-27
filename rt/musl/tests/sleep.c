/* Sleeping, against native: what a program sees that does not depend on the real time elapsed. Each sleep
 * returns at once with nothing remaining, later clock reads are at least the duration later, a CPU-time
 * clock does not count the sleep, and invalid arguments give Linux's errors in Linux's order. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sys/time.h>

static int64_t ns(clockid_t id)
{
    struct timespec t;
    if (clock_gettime(id, &t)) return -1;
    return (int64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}

static long raw(long id, long flags, const struct timespec *request)
{
    long r = syscall(SYS_clock_nanosleep, id, flags, request, (void *)0);
    return r < 0 ? -errno : r;
}

static void sleeps(const char *name, int returned, struct timespec left)
{
    printf("%s: %d, remaining %lld %ld\n", name, returned, (long long)left.tv_sec, left.tv_nsec);
}

int main(void)
{
    /* The remaining time is written only when a signal interrupts the sleep. */
    struct timespec left = {77, 77};
    struct timespec quarter = {0, 250000000};
    int64_t before = ns(CLOCK_MONOTONIC);
    sleeps("nanosleep a quarter second", nanosleep(&quarter, &left), left);
    int64_t after = ns(CLOCK_MONOTONIC);
    printf("monotonic advanced by the quarter: %d\n", after - before >= 250000000);
    printf("sleep(0): %u, sleep(1): %u\n", sleep(0), sleep(1));
    time_t seconds = time(0);
    printf("sleep(2): %u\n", sleep(2));
    printf("time advanced by two seconds: %d\n", time(0) - seconds >= 2);
    struct timeval tv0, tv1;
    gettimeofday(&tv0, 0);
    printf("usleep(1500): %d\n", usleep(1500));
    gettimeofday(&tv1, 0);
    printf("gettimeofday advanced: %d\n",
           (tv1.tv_sec - tv0.tv_sec) * 1000000 + (tv1.tv_usec - tv0.tv_usec) >= 1500);

    /* Absolute sleeps end at the target; one in the past returns at once. */
    struct timespec target;
    clock_gettime(CLOCK_MONOTONIC, &target);
    target.tv_nsec += 100000000;
    if (target.tv_nsec >= 1000000000) target.tv_nsec -= 1000000000, ++target.tv_sec;
    printf("absolute monotonic: %d\n", clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &target, 0));
    printf("reached the target: %d\n", ns(CLOCK_MONOTONIC) >= (int64_t)target.tv_sec * 1000000000 + target.tv_nsec);
    clock_gettime(CLOCK_REALTIME, &target);
    target.tv_sec += 1;
    printf("absolute realtime: %d\n", clock_nanosleep(CLOCK_REALTIME, TIMER_ABSTIME, &target, 0));
    printf("reached the target: %d\n", ns(CLOCK_REALTIME) >= (int64_t)target.tv_sec * 1000000000 + target.tv_nsec);
    struct timespec past = {1, 0};
    printf("absolute in the past: %d\n", clock_nanosleep(CLOCK_REALTIME, TIMER_ABSTIME, &past, 0));
    struct timespec tenth = {0, 100000000};
    printf("boottime %d, tai %d\n", clock_nanosleep(CLOCK_BOOTTIME, 0, &tenth, 0),
           clock_nanosleep(CLOCK_TAI, 0, &tenth, 0));
    /* Only TIMER_ABSTIME among the flags has a meaning. */
    printf("other flags: %ld %ld\n", raw(CLOCK_MONOTONIC, 2, &tenth), raw(CLOCK_MONOTONIC, -2, &tenth));

    /* CPU time does not advance while the process sleeps. */
    int64_t cpu = ns(CLOCK_PROCESS_CPUTIME_ID);
    clock_t ticks = clock();
    sleep(1);
    printf("cpu time unchanged by a second's sleep: %d %d\n", ns(CLOCK_PROCESS_CPUTIME_ID) - cpu < 500000000,
           clock() - ticks < CLOCKS_PER_SEC / 2);
    struct timespec zero = {0, 0};
    printf("cpu clock, zero and past: %ld %ld %ld\n", raw(CLOCK_PROCESS_CPUTIME_ID, 0, &zero),
           raw(CLOCK_PROCESS_CPUTIME_ID, TIMER_ABSTIME, &zero), raw(-6, 0, &zero));

    /* Errors: the clock id, then the request's address, then its value, then the clock's own checks. */
    struct timespec bad_nsec = {0, 1000000000}, negative_nsec = {0, -1}, negative_sec = {-1, 0};
    /* Unreadable requests: null, the lowest addresses, one that wraps and one beyond any memory. Linux copies the
     * whole request before it checks the value. */
    const struct timespec *unreadable[] = {0, (void *)8, (void *)-1L, (void *)(1L << 62), (void *)-8L};
    for (unsigned i = 0; i < sizeof unreadable / sizeof *unreadable; ++i) {
        errno = 0;
        int returned = nanosleep(unreadable[i], &left);
        printf("nanosleep unreadable %u: %d %d\n", i, returned, errno);
        printf("  clock_nanosleep: %d, raw monotonic %ld\n", clock_nanosleep(CLOCK_REALTIME, 0, unreadable[i], 0),
               raw(CLOCK_MONOTONIC, TIMER_ABSTIME, unreadable[i]));
    }
    const struct timespec *invalid[] = {&bad_nsec, &negative_nsec, &negative_sec};
    for (unsigned i = 0; i < sizeof invalid / sizeof *invalid; ++i) {
        errno = 0;
        int returned = nanosleep(invalid[i], &left);
        printf("nanosleep invalid %u: %d %d\n", i, returned, errno);
    }
    printf("absolute -1 s: %d\n", clock_nanosleep(CLOCK_REALTIME, TIMER_ABSTIME, &negative_sec, 0));
    printf("library: thread cpu %d, raw %d, unknown %d\n",
           clock_nanosleep(CLOCK_THREAD_CPUTIME_ID, 0, &zero, 0), clock_nanosleep(CLOCK_MONOTONIC_RAW, 0, &zero, 0),
           clock_nanosleep(99, 0, &zero, 0));
    long ids[] = {3, 4, 5, 6, 10, 12, 99, -1, -2, -3, -5, -10, -1073741830};
    for (unsigned i = 0; i < sizeof ids / sizeof *ids; ++i)
        printf("id %ld: %ld %ld %ld\n", ids[i], raw(ids[i], 0, &zero), raw(ids[i], 0, 0), raw(ids[i], 0, &bad_nsec));
    printf("alarm clocks, bad requests: %ld %ld %ld %ld\n", raw(CLOCK_REALTIME_ALARM, 0, 0),
           raw(CLOCK_REALTIME_ALARM, 0, &bad_nsec), raw(CLOCK_BOOTTIME_ALARM, 0, 0),
           raw(CLOCK_BOOTTIME_ALARM, 0, &negative_sec));
    /* The alarm clocks check their flags after the request and before the capability to wake the system. */
    printf("alarm clocks, other flags:");
    long flags[] = {2, 3, -1, -2};
    for (unsigned i = 0; i < sizeof flags / sizeof *flags; ++i)
        printf(" %ld %ld", raw(CLOCK_REALTIME_ALARM, flags[i], &zero), raw(CLOCK_BOOTTIME_ALARM, flags[i], &tenth));
    printf("; bad requests first: %ld %ld\n", raw(CLOCK_REALTIME_ALARM, 2, (void *)-1L),
           raw(CLOCK_BOOTTIME_ALARM, -1, &negative_nsec));
    printf("remaining untouched: %lld %ld\n", (long long)left.tv_sec, left.tv_nsec);
    return 0;
}
