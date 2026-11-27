/* A positive sleep on the process's CPU-time clock: CPU time does not advance while the process sleeps, so
 * natively this never returns; the device refuses it by name (`unreachable`). */
#include <time.h>
int main(void) {
    struct timespec t = {0, 1};
    return clock_nanosleep(CLOCK_PROCESS_CPUTIME_ID, 0, &t, 0);
}
