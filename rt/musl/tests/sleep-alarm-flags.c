/* The alarm clocks' flags, against native: flags other than TIMER_ABSTIME with a valid request are EINVAL on a host
 * with a real-time clock device and on the device; a host without one answers EOPNOTSUPP (test.py compares with
 * that answer read as EINVAL, rt/musl/README.md, "Sleeping"). */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>
#include <sys/syscall.h>

static long raw(long id, long flags, const struct timespec *request)
{
    long r = syscall(SYS_clock_nanosleep, id, flags, request, (void *)0);
    return r < 0 ? -errno : r;
}

int main(void)
{
    struct timespec zero = {0, 0}, tenth = {0, 100000000};
    long flags[] = {2, 3, -1, -2};
    printf("alarm clocks, other flags:");
    for (unsigned i = 0; i < sizeof flags / sizeof *flags; ++i)
        printf(" %ld %ld", raw(CLOCK_REALTIME_ALARM, flags[i], &zero), raw(CLOCK_BOOTTIME_ALARM, flags[i], &tenth));
    printf("\n");
    return 0;
}
