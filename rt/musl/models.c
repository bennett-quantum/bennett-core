/* Adapted from bennett-runtimes src/device/models.c; see CARRIED.md.
 * Inputs are immutable program data; consumption is ordinary journalled memory. */
#include "devices.h"
#include <string.h>

extern const uint64_t __bennett_clock_start;
extern const unsigned char __bennett_entropy_image[];
extern const uint64_t __bennett_entropy_size;
/* The run's clock: the start (an input of the run) plus the number of earlier reads, plus, for the clocks that
 * count while the process sleeps, the nanoseconds slept so far. All three counters are ordinary memory, so undo
 * restores them. */
static uint64_t clock_reads, clock_slept, entropy_pos;

uint64_t __bennett_clock_now(int cpu)
{
    /* unsigned nanoseconds, wrapping modulo 2^64 */
    return __bennett_clock_start + clock_reads + (cpu ? 0 : clock_slept);
}

uint64_t __bennett_clock_read(int cpu)
{
    uint64_t value = __bennett_clock_now(cpu);
    ++clock_reads;
    return value;
}

void __bennett_clock_sleep(uint64_t nanoseconds)
{
    clock_slept += nanoseconds;
}

void __bennett_entropy_check(uint64_t length)
{
    /* Exhaustion faults before touching destination or position, including vector reads. */
    if(entropy_pos > __bennett_entropy_size || length > __bennett_entropy_size-entropy_pos)
        __builtin_trap();
}

void __bennett_entropy_read(void *destination, uint64_t length)
{
    __bennett_entropy_check(length);
    if(!length) return;
    if(!destination) __builtin_trap();
    memcpy(destination, __bennett_entropy_image+entropy_pos, length);
    entropy_pos += length;
}
