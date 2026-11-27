/* Timers deliver a signal asynchronously and are refused by name (`unreachable`) until the route in
 * docs/DESIGN.md ("Sleeping") is built. */
#include <unistd.h>
int main(void) { return (int)alarm(1); }
