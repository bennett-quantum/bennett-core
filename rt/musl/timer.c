/* timer_create on the device: build.py compiles this file in place of musl's src/time/timer_create.c, so the
 * member is pulled from the archive only when a program calls timer_create and does not define its own, as
 * musl's is. Timers are refused by name (rt/musl/README.md, "Sleeping"): every call stops at run time with
 * `unreachable`, as the other timer calls do at their unimplemented system calls. musl's own member also links
 * the thread that a SIGEV_THREAD timer notifies, whose exit (__unmapself) is inline assembly the front does not
 * translate. */
#include <time.h>

int timer_create(clockid_t clock, struct sigevent *restrict event, timer_t *restrict timer)
{
    (void)clock; (void)event; (void)timer;
    __builtin_trap();
}
