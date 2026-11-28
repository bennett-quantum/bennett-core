/* A sleep request beyond device memory, below the top of the user address space, stops with the device's
 * `out_of_bounds`: the runtime does not know where memory ends, so it reads the request and the device's bound
 * stops the read (natively EFAULT; rt/musl/README.md, "Sleeping"). A request above the top is EFAULT on both
 * (efault-time, nanosleep-above-top). */
#include <stdio.h>
#include <time.h>

int main(void)
{
    puts("before the request");
    fflush(stdout);
    return nanosleep((const struct timespec *)(1UL << 40), 0);
}
