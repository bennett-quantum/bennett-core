#ifndef BENNETT_COPY_H
#define BENNETT_COPY_H
/* How a system call reads and writes the program's memory (rt/musl/README.md, "Structures in system calls").
 * As Linux copies each input structure whole before it checks any value and writes each output once the result
 * is known, an entry copies every input in with __bennett_copy_in, checks only the copy, and writes every output
 * with __bennett_copy_out after its inputs are read and checked; no entry reads a field of the program's
 * structure in place. */
#include <stdint.h>

/* A range Linux answers EFAULT for everywhere and the device can tell: one that starts below address 16 (the
 * device has no object there; Linux never maps the lowest page) or wraps past the top of the address space. Device
 * memory is the module's bytes [0, M), static data from address 16 and the stack down from M; natively readable
 * means mapped, an unrelated address space, so the two agree on these ranges only. An empty range is never
 * refused: Linux copies nothing for it. */
static inline int __bennett_user_refused(const void *p, uint64_t n)
{
    uintptr_t address = (uintptr_t)p;
    return n && (address < 16 || n - 1 > UINTPTR_MAX - address);
}

/* Copies the n bytes at from, in the program's memory, into the runtime's own to; returns 0 (EFAULT) for a refused
 * range. Any other range is read byte by byte, every byte before the caller looks at any of them, and one that
 * reaches the end of device memory or beyond stops the run in that read with the device's `out_of_bounds`, since
 * the runtime does not know M. The loads are volatile so that no byte's load can be dropped or moved after a check
 * of the copy. */
static inline int __bennett_copy_in(void *to, const void *from, uint64_t n)
{
    if (__bennett_user_refused(from, n)) return 0;
    const volatile unsigned char *source = from;
    unsigned char *copy = to;
    for (uint64_t i = 0; i < n; ++i) copy[i] = source[i];
    return 1;
}

/* Copies the n bytes of the runtime's own from to the program's memory at to; returns 0 (EFAULT) for a refused range,
 * with nothing written. Any other range is written byte by byte, in order; one reaching the end of device memory
 * stops the run there (natively the bytes before an unmapped one are written and the call answers EFAULT). */
static inline int __bennett_copy_out(void *to, const void *from, uint64_t n)
{
    if (__bennett_user_refused(to, n)) return 0;
    volatile unsigned char *target = to;
    const unsigned char *value = from;
    for (uint64_t i = 0; i < n; ++i) target[i] = value[i];
    return 1;
}
#endif
