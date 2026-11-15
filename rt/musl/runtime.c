/* The single-threaded device boundary. Startup and exit sequencing belong to unmodified musl. */
#define _GNU_SOURCE
#include "pthread_impl.h"
#include "ksigaction.h"
#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>

#ifndef BENNETT_ARENA_BYTES
#define BENNETT_ARENA_BYTES (1 << 20)
#endif
_Static_assert(BENNETT_ARENA_BYTES >= 4096 && BENNETT_ARENA_BYTES % 4096 == 0,
               "the arena is a positive whole number of 4096-byte pages");

void __bennett_effect(char);
_Noreturn void __bennett_exit(int);
struct pthread *__bennett_thread_pointer;
int __set_thread_area(void *pointer)
{
    __bennett_thread_pointer = pointer;
    return 0;
}

static unsigned char arena[BENNETT_ARENA_BYTES] __attribute__((aligned(4096)));
static size_t brk_used, mmap_used;
static unsigned open_fds = 7;
static unsigned long signal_mask;

static long write_console(int fd, const void *buffer, size_t length)
{
    /* There is one console. stderr is explicitly unsupported, never silently discarded. */
    if (fd < 0 || fd > 2 || !(open_fds & (1u << fd))) return -EBADF;
    if (fd == 2) __builtin_trap();
    if (fd != 1) return -EBADF;
    if (length > LONG_MAX) return -EINVAL;
    for (size_t i = 0; i < length; ++i) __bennett_effect(((const char *)buffer)[i]);
    return length;
}

static int arena_range(uintptr_t address, size_t length)
{
    uintptr_t base = (uintptr_t)arena;
    return address >= base && address - base <= BENNETT_ARENA_BYTES &&
           length <= BENNETT_ARENA_BYTES - (address - base);
}

long __bennett_syscall(long number, long a1, long a2, long a3, long a4, long a5, long a6)
{
    (void)a5;
    (void)a6;
    switch (number) {
    case SYS_write:
        return write_console(a1, (const void *)a2, a3);
    case SYS_writev: {
        if (a3 < 0 || a3 > 1024) return -EINVAL;
        const struct iovec *iov = (const void *)a2;
        long total = 0;
        for (long i = 0; i < a3; ++i) {
            if (iov[i].iov_len > (size_t)LONG_MAX - total) return -EINVAL;
            long count = write_console(a1, iov[i].iov_base, iov[i].iov_len);
            if (count < 0) return count;
            total += count;
        }
        return total;
    }
    case SYS_read:
        return a1 == 0 && (open_fds & 1) ? 0 : -EBADF; /* Empty stdin. */
    case SYS_ioctl:
        return -ENOTTY; /* Fully buffered stdout, as for a native pipe. */
    case SYS_open:
    case SYS_openat:
        __builtin_trap(); /* No filesystem. */
    case SYS_close:
        if (a1 < 0 || a1 > 2 || !(open_fds & (1u << a1))) return -EBADF;
        open_fds &= ~(1u << a1);
        return 0;
    case SYS_lseek:
        return a1 >= 0 && a1 <= 2 ? -ESPIPE : -EBADF;
    case SYS_brk: {
        uintptr_t base = (uintptr_t)arena, want = a1;
        if (want >= base && want - base <= BENNETT_ARENA_BYTES - mmap_used) brk_used = want - base;
        return base + brk_used;
    }
    case SYS_mmap: {
        if (a1 || a3 != (PROT_READ | PROT_WRITE) || a4 != (MAP_PRIVATE | MAP_ANONYMOUS) || a6)
            __builtin_trap();
        size_t length = a2;
        if (!length) return -EINVAL;
        if (length > BENNETT_ARENA_BYTES || length > SIZE_MAX - 4095) return -ENOMEM;
        length = (length + 4095) & ~(size_t)4095;
        if (length > BENNETT_ARENA_BYTES - mmap_used - brk_used) return -ENOMEM;
        mmap_used += length;
        return (long)(arena + BENNETT_ARENA_BYTES - mmap_used);
    }
    case SYS_munmap:
        if ((a1 & 4095) || !a2 || !arena_range(a1, a2)) return -EINVAL;
        return 0; /* Monotone arena: valid released allocations are never reused. */
    case SYS_madvise:
        if ((a1 & 4095) || !arena_range(a1, a2)) return -EINVAL;
        if (a3 == MADV_DONTNEED) {
            for (size_t i = 0; i < (size_t)a2; ++i) ((unsigned char *)a1)[i] = 0;
            return 0;
        }
        __builtin_trap();
    case SYS_mprotect:
        if ((a1 & 4095) || !arena_range(a1, a2)) return -EINVAL;
        if (a3 != (PROT_READ | PROT_WRITE)) __builtin_trap();
        return 0;
    case SYS_mremap:
        return -ENOMEM;
    case SYS_rt_sigprocmask: {
        if (a4 != sizeof(unsigned long)) return -EINVAL;
        unsigned long desired = a2 ? *(const unsigned long *)a2 : 0;
        if (a2 && a1 != SIG_BLOCK && a1 != SIG_UNBLOCK && a1 != SIG_SETMASK) return -EINVAL;
        if (a3) *(unsigned long *)a3 = signal_mask;
        if (a2) {
            if (a1 == SIG_BLOCK) signal_mask |= desired;
            else if (a1 == SIG_UNBLOCK) signal_mask &= ~desired;
            else signal_mask = desired;
            signal_mask &= ~((1ul << (SIGKILL - 1)) | (1ul << (SIGSTOP - 1)));
        }
        return 0;
    }
    case SYS_rt_sigaction:
        /* No asynchronous signals; installing handlers is explicitly unsupported. */
        if (a4 != sizeof(unsigned long)) return -EINVAL;
        if (a2) __builtin_trap();
        if (a3) for (size_t i = 0; i < sizeof(struct k_sigaction); ++i) ((unsigned char *)a3)[i] = 0;
        return 0;
    case SYS_gettid:
    case SYS_getpid:
    case SYS_set_tid_address:
        return 1;
    case SYS_tkill:
    case SYS_tgkill:
    case SYS_kill:
        if (a1 != 1 || (number == SYS_tgkill && a2 != 1) ||
            (number == SYS_tgkill ? a3 : a2) != SIGABRT || (signal_mask & (1ul << (SIGABRT - 1))))
            __builtin_trap();
        __bennett_exit(128 + SIGABRT);
    case SYS_exit:
    case SYS_exit_group:
        __bennett_exit((int)a1);
    }
    __builtin_trap();
}
