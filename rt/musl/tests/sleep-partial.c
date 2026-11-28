/* A sleep request whose seconds lie in device memory and whose nanoseconds lie past its end stops with the
 * device's `out_of_bounds` whatever the seconds hold (natively EFAULT): the runtime copies the whole request
 * before it checks a field (rt/musl/README.md, "Sleeping"). test.py runs the entry partial_entry with a request
 * address near the memory size M, the fields to write there (seconds, and nanoseconds if fields is 2) and the
 * call (0 nanosleep, 1 clock_nanosleep, 2 the raw system call). The top bytes of memory lie above the entry's
 * frame, so the entry may write them. */
#define _GNU_SOURCE
#include <errno.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static long sleep_on(const void *request, long which)
{
    if (which == 0)
        return nanosleep(request, 0) ? errno : 0;
    if (which == 1)
        return clock_nanosleep(CLOCK_MONOTONIC, 0, request, 0);
    return syscall(SYS_clock_nanosleep, CLOCK_MONOTONIC, 0, request, 0) ? errno : 0;
}

long partial_entry(unsigned long address, long seconds, long nanoseconds, long fields, long which)
{
    volatile long *request = (volatile long *)address;
    request[0] = seconds;
    if (fields == 2)
        request[1] = nanoseconds;
    return sleep_on((const void *)address, which);
}

int main(void)
{
    return 0;
}
