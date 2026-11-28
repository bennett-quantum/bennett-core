/* A sleep request in a frame above the C library's start-up: the device runs the entry caller_entry, whose local
 * main reads; natively main is the entry and the request its own local. Both sleep to a past target and return 0. */
#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <time.h>

static const struct timespec *saved;

static int check(void)
{
    int returned = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, saved, 0);
    printf("request in the caller's frame: %d\n", returned);
    return returned;
}

#ifdef NATIVE
int main(void)
{
    const struct timespec request = {0, 0};
    saved = &request;
    return check();
}
#else
extern int __bennett_entry(void);

int caller_entry(void)
{
    const struct timespec request = {0, 0};
    saved = &request;
    return __bennett_entry();
}

int main(void)
{
    return check();
}
#endif
