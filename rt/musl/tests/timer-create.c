/* timer_create is refused by name (`unreachable`) at run time like the other timer calls, until the route in
 * docs/DESIGN.md is built (rt/musl/README.md, "Sleeping"). */
#include <time.h>
int main(void)
{
    timer_t timer;
    return timer_create(CLOCK_MONOTONIC, 0, &timer);
}
