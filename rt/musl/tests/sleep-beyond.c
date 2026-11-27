/* A sleep of 2^64 nanoseconds or more lies beyond the clock's range (natively it would not end in any run);
 * the device refuses it by name (`unreachable`). */
#include <time.h>
int main(void) {
    struct timespec t = {18446744074, 0};
    return nanosleep(&t, 0);
}
