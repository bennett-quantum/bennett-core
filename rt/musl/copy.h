#ifndef BENNETT_COPY_H
#define BENNETT_COPY_H
/* How a system call reads and writes the program's memory (rt/musl/README.md, "Structures in system calls").
 * As Linux copies each input structure whole before it checks any value and writes each output once the result
 * is known, an entry copies every input in with __bennett_copy_in, checks only the copy, and writes every output
 * with __bennett_copy_out after its inputs are read and checked; no entry reads a field of the program's
 * structure in place. A data buffer is checked in Linux's two stages: its range where the call checks it
 * (__bennett_user_range), and its first byte where bytes are transferred (__bennett_user_unmapped). A path string
 * is copied whole by __bennett_copy_path before the call looks at it. */
#include <errno.h>
#include <stdint.h>

/* The top of the user address space Linux checks ranges against on x86-64 with four-level page tables
 * (TASK_SIZE_MAX, 2^47 minus one page). Device memory lies far below it. */
#define BENNETT_USER_TOP 0x7ffffffff000ull

/* Linux's cap on the bytes one read, write or getrandom transfers (MAX_RW_COUNT: INT_MAX rounded down to a page);
 * a vector's lengths are capped so that their total stays within it. */
#define BENNETT_MAX_RW_COUNT 0x7ffff000ull

/* Linux's access_ok: a range is acceptable unless it wraps or ends above BENNETT_USER_TOP, whatever is mapped; an
 * empty range at an address above the top is refused too. Natively a refused range is EFAULT before any byte is
 * touched, so the device answers the same. */
static inline int __bennett_user_range(const void *p, uint64_t n)
{
    uintptr_t address = (uintptr_t)p;
    return address <= BENNETT_USER_TOP && n <= BENNETT_USER_TOP - address;
}

/* Whether touching the first byte of an acceptable nonempty range faults where the device can tell: it starts
 * below address 16 (the device has no object there; Linux never maps the lowest page). Natively that fault
 * transfers nothing and the call answers EFAULT. Device memory is the module's bytes [0, M), static data from
 * address 16 and the stack down from M; natively readable means mapped, an unrelated address space, so the two
 * agree on these low ranges and on refused ranges only. */
static inline int __bennett_user_unmapped(const void *p, uint64_t n)
{
    return n && (uintptr_t)p < 16;
}

/* Whether a structure of n bytes at p can be copied: copy_from_user and copy_to_user check the range, then touch
 * every byte. A copy of no bytes always succeeds. */
static inline int __bennett_user_copyable(const void *p, uint64_t n)
{
    return !n || (__bennett_user_range(p, n) && !__bennett_user_unmapped(p, n));
}

/* Copies the n bytes at from, in the program's memory, into the runtime's own to; returns 0 (EFAULT) for a refused
 * range. Any other range is read byte by byte, every byte before the caller looks at any of them, and one that
 * reaches the end of device memory or beyond stops the run in that read with the device's `out_of_bounds`, since
 * the runtime does not know M. The loads are volatile so that no byte's load can be dropped or moved after a check
 * of the copy. */
static inline int __bennett_copy_in(void *to, const void *from, uint64_t n)
{
    if (!__bennett_user_copyable(from, n)) return 0;
    const volatile unsigned char *source = from;
    unsigned char *copy = to;
    for (uint64_t i = 0; i < n; ++i) copy[i] = source[i];
    return 1;
}

/* Linux's PATH_MAX: a path string with its NUL fits in this many bytes. */
#define BENNETT_PATH_MAX 4096

/* Copies the path string at from, up to and including its NUL, into the runtime's own to, as Linux's getname
 * (strncpy_from_user) does; returns 0 or the error: EFAULT for a string starting below 16, at the top of the user
 * address space or above it, or reaching the top before its NUL; ENAMETOOLONG for BENNETT_PATH_MAX bytes without
 * a NUL, whatever follows; ENOENT for an empty string. Bytes are read one by one up to the first NUL, never past
 * it; one at the end of device memory or beyond stops the run there with `out_of_bounds`, as for a structure. */
static inline int __bennett_copy_path(char to[BENNETT_PATH_MAX], const char *from)
{
    uintptr_t address = (uintptr_t)from;
    if (address < 16 || address >= BENNETT_USER_TOP) return EFAULT;
    const volatile char *source = from;
    for (uint64_t i = 0; i < BENNETT_PATH_MAX; ++i) {
        if (i == BENNETT_USER_TOP - address) return EFAULT;
        to[i] = source[i];
        if (!to[i]) return i ? 0 : ENOENT;
    }
    return ENAMETOOLONG;
}

/* Copies the n bytes of the runtime's own from to the program's memory at to; returns 0 (EFAULT) for a refused range,
 * with nothing written. Any other range is written byte by byte, in order; one reaching the end of device memory
 * stops the run there (natively the bytes before an unmapped one are written and the call answers EFAULT). */
static inline int __bennett_copy_out(void *to, const void *from, uint64_t n)
{
    if (!__bennett_user_copyable(to, n)) return 0;
    volatile unsigned char *target = to;
    const unsigned char *value = from;
    for (uint64_t i = 0; i < n; ++i) target[i] = value[i];
    return 1;
}
#endif
