/* A sleep on an alarm clock: natively EPERM without the capability to wake the system, a sleep with it, so the
 * result is a fact of the host; the device refuses it by name (`unreachable`). */
#define _GNU_SOURCE
#include <time.h>
int main(void) {
    struct timespec t = {0, 1};
    return clock_nanosleep(CLOCK_REALTIME_ALARM, 0, &t, 0);
}
