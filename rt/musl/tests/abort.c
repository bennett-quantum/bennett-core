/* abort terminates with SIGABRT's status after its handler returns, without flushing stdio. */
#include <signal.h>
#include <stdlib.h>
static void ignore(int sig) { (void)sig; }
int main(void) {
    signal(SIGABRT, ignore);
    abort();
}
