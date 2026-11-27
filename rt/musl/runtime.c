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
#include <time.h>
#include "devices.h"

#ifndef BENNETT_ARENA_BYTES
#define BENNETT_ARENA_BYTES (1 << 20)
#endif
#define PAGE 4096
_Static_assert(BENNETT_ARENA_BYTES >= PAGE && BENNETT_ARENA_BYTES % PAGE == 0,
               "the arena is a positive whole number of 4096-byte pages");

void __bennett_effect(char);
_Noreturn void __bennett_exit(int);
/* Ends the run with the named fault `out_of_memory`: the arena cannot hold a request. */
_Noreturn void __bennett_out_of_memory(void);
struct pthread *__bennett_thread_pointer;
int __set_thread_area(void *pointer)
{
    __bennett_thread_pointer = pointer;
    return 0;
}

static unsigned char arena[BENNETT_ARENA_BYTES] __attribute__((aligned(PAGE)));
static size_t brk_used, mmap_used;

static size_t page_up(size_t n)
{
    return (n + PAGE - 1) & ~(size_t)(PAGE - 1);
}

static void zero(unsigned char *p, size_t n)
{
    for (size_t i = 0; i < n; ++i) p[i] = 0;
}

/* The arena: brk grows from the bottom, anonymous mappings from the top; nothing is reused. A growth or a
 * fresh mapping it cannot hold is the fault `out_of_memory`, never an error result, so no allocation
 * function returns null for exhaustion (musl's own arithmetic rejections, such as `malloc(SIZE_MAX)`, never
 * reach the system call). */
static int arena_range(uintptr_t address, size_t length)
{
    uintptr_t base = (uintptr_t)arena;
    return address >= base && address - base <= BENNETT_ARENA_BYTES &&
           length <= BENNETT_ARENA_BYTES - (address - base);
}

/* A range already handed out by brk or by an earlier mapping. */
static int arena_in_use(uintptr_t address, size_t length)
{
    uintptr_t base = (uintptr_t)arena, top = base + BENNETT_ARENA_BYTES;
    if (!arena_range(address, length)) return 0;
    return address + length <= base + page_up(brk_used) || address >= top - mmap_used;
}

static long map(long address, size_t length, long prot, long flags)
{
    /* Protection is recorded nowhere: a program that passes natively never touches memory it may not. */
    if (prot & ~(long)(PROT_READ | PROT_WRITE)) __builtin_trap();
    if ((flags & ~(long)MAP_FIXED) != (MAP_PRIVATE | MAP_ANONYMOUS)) __builtin_trap();
    if (!length) return -EINVAL;
    if (flags & MAP_FIXED) {
        /* A fresh private mapping over part of the arena already in use (musl's heap guard page). */
        if (length > BENNETT_ARENA_BYTES) return -ENOMEM;
        length = page_up(length);
        if (address & (PAGE - 1)) return -EINVAL;
        if (!arena_in_use(address, length)) __builtin_trap();
        zero((unsigned char *)address, length);
        return address;
    }
    /* Without MAP_FIXED the address is only a hint. The free pages lie between the break and the lowest
     * mapping; their size is a whole number of pages, so the rounded length fits whenever the length does. */
    if (length > BENNETT_ARENA_BYTES - mmap_used - page_up(brk_used)) __bennett_out_of_memory();
    mmap_used += page_up(length);
    return (long)(arena + BENNETT_ARENA_BYTES - mmap_used);
}

static long set_break(uintptr_t want)
{
    uintptr_t base = (uintptr_t)arena;
    /* The query brk(0), or an address below the arena: the current break, as Linux answers a failed brk. */
    if (want < base) return base + brk_used;
    /* A break the arena cannot hold is always a growth, since the current break fits. */
    if (want - base > BENNETT_ARENA_BYTES - mmap_used) __bennett_out_of_memory();
    size_t old = brk_used;
    brk_used = want - base;
    /* Pages given back are fresh zero pages if the break grows over them again. */
    if (page_up(brk_used) < page_up(old)) zero(arena + page_up(brk_used), page_up(old) - page_up(brk_used));
    return base + brk_used;
}

/* Signals. There are no asynchronous signals: only those the program sends itself, delivered as the
 * kernel delivers them, before the system call that unblocks or raises them returns. */
#define SIGNALS 64
#define FIRST_REALTIME_SIGNAL 32 /* the kernel's; musl reserves the first three */
static unsigned long signal_mask, pending;
static struct k_sigaction actions[SIGNALS];
static ucontext_t context;

static unsigned long bit(int sig)
{
    return 1ul << (sig - 1);
}

static void default_action(int sig)
{
    switch (sig) {
    case SIGCHLD:
    case SIGCONT:
    case SIGURG:
    case SIGWINCH:
        return; /* ignored by default */
    case SIGSTOP:
    case SIGTSTP:
    case SIGTTIN:
    case SIGTTOU:
        __builtin_trap(); /* a stopped process has no device counterpart */
    }
    /* Termination: no stdio flush, no atexit handlers; the status a shell reports. */
    __bennett_exit(128 + sig);
}

static void deliver(void)
{
    for (unsigned long ready; (ready = pending & ~signal_mask);) {
        int sig = __builtin_ctzl(ready) + 1;
        pending &= ~bit(sig);
        struct k_sigaction action = actions[sig - 1];
        if (action.handler == SIG_IGN) continue;
        if (action.handler == SIG_DFL) {
            default_action(sig);
            continue;
        }
        if (action.flags & SA_RESETHAND) {
            actions[sig - 1].handler = SIG_DFL;
            actions[sig - 1].flags &= ~SA_SIGINFO;
        }
        unsigned long saved = signal_mask;
        signal_mask |= (action.mask[0] | (unsigned long)action.mask[1] << 32) |
                       (action.flags & SA_NODEFER ? 0 : bit(sig));
        signal_mask &= ~(bit(SIGKILL) | bit(SIGSTOP));
        if (action.flags & SA_SIGINFO) {
            siginfo_t info = {0};
            info.si_signo = sig;
            info.si_code = SI_TKILL;
            info.si_pid = 1;
            ((void (*)(int, siginfo_t *, void *))action.handler)(sig, &info, &context);
        } else {
            action.handler(sig);
        }
        signal_mask = saved;
    }
}

static long send_self(long sig)
{
    if (sig < 0 || sig > SIGNALS) return -EINVAL;
    if (!sig) return 0; /* existence check */
    /* A real-time signal queues; a second one pending would be lost here. */
    if (sig >= FIRST_REALTIME_SIGNAL && (pending & bit(sig))) __builtin_trap();
    pending |= bit(sig);
    deliver();
    return 0;
}

/* Timers are refused by name (rt/musl/README.md, "Sleeping"). musl's timer_create also links its thread
 * notification, whose thread exit (__unmapself) is inline assembly the front does not translate, so the
 * runtime defines it: every call stops at run time, as the raw system call and the other timer calls do. */
int timer_create(clockid_t clock, struct sigevent *restrict event, timer_t *restrict timer)
{
    (void)clock; (void)event; (void)timer;
    __builtin_trap();
}

long __bennett_syscall(long number, long a1, long a2, long a3, long a4, long a5, long a6)
{
    (void)a5;
    (void)a6;
    switch (number) {
    case SYS_read: case SYS_write: case SYS_readv: case SYS_writev:
    case SYS_open: case SYS_openat: case SYS_creat: case SYS_close:
    case SYS_dup: case SYS_dup2: case SYS_dup3: case SYS_lseek: case SYS_ioctl:
    case SYS_pread64: case SYS_pwrite64:
    case SYS_stat: case SYS_fstat: case SYS_lstat: case SYS_access:
    case SYS_readlink: case SYS_readlinkat:
    case SYS_fcntl: case SYS_fsync: case SYS_fdatasync: case SYS_truncate: case SYS_ftruncate:
    case SYS_getcwd: case SYS_chdir: case SYS_fchdir: case SYS_rename: case SYS_mkdir:
    case SYS_rmdir: case SYS_unlink: case SYS_chmod: case SYS_mkdirat:
    case SYS_umask: case SYS_newfstatat: case SYS_unlinkat: case SYS_faccessat:
    case SYS_clock_gettime: case SYS_clock_getres: case SYS_gettimeofday: case SYS_time:
    case SYS_nanosleep: case SYS_clock_nanosleep:
    case SYS_getrandom:
        return __bennett_device_syscall(number, a1, a2, a3, a4, a5, a6);
    case SYS_brk:
        return set_break(a1);
    case SYS_mmap:
        return map(a1, a2, a3, a4);
    case SYS_munmap:
        if ((a1 & (PAGE - 1)) || !a2 || !arena_range(a1, a2)) return -EINVAL;
        return 0; /* Monotone arena: valid released allocations are never reused. */
    case SYS_madvise:
        if ((a1 & (PAGE - 1)) || !arena_range(a1, page_up(a2))) return -EINVAL;
        switch (a3) {
        case MADV_DONTNEED:
            zero((unsigned char *)a1, page_up(a2)); /* whole pages, as the kernel drops them */
            return 0;
        case MADV_NORMAL:
        case MADV_RANDOM:
        case MADV_SEQUENTIAL:
        case MADV_WILLNEED:
            return 0; /* hints that change no contents */
        }
        __builtin_trap();
    case SYS_mprotect:
        if ((a1 & (PAGE - 1)) || !arena_range(a1, page_up(a2))) return -EINVAL;
        if (a3 & ~(long)(PROT_READ | PROT_WRITE)) __builtin_trap();
        return 0; /* not enforced, as for map */
    case SYS_mremap:
        return -ENOMEM;
    case SYS_rt_sigprocmask: {
        if (a4 != SIGNALS / 8) return -EINVAL;
        if (a2 && a1 != SIG_BLOCK && a1 != SIG_UNBLOCK && a1 != SIG_SETMASK) return -EINVAL;
        unsigned long desired = a2 ? *(const unsigned long *)a2 : 0;
        if (a3) *(unsigned long *)a3 = signal_mask;
        if (a2) {
            if (a1 == SIG_BLOCK) signal_mask |= desired;
            else if (a1 == SIG_UNBLOCK) signal_mask &= ~desired;
            else signal_mask = desired;
            signal_mask &= ~(bit(SIGKILL) | bit(SIGSTOP));
            deliver();
        }
        return 0;
    }
    case SYS_rt_sigpending:
        if (a2 != SIGNALS / 8) return -EINVAL;
        *(unsigned long *)a1 = pending;
        return 0;
    case SYS_rt_sigaction: {
        if (a4 != SIGNALS / 8 || a1 < 1 || a1 > SIGNALS) return -EINVAL;
        if (a2 && (a1 == SIGKILL || a1 == SIGSTOP)) return -EINVAL;
        if (a3) *(struct k_sigaction *)a3 = actions[a1 - 1];
        if (a2) {
            actions[a1 - 1] = *(const struct k_sigaction *)a2;
            /* Ignoring a signal discards it if pending. */
            if (actions[a1 - 1].handler == SIG_IGN) pending &= ~bit(a1);
        }
        return 0;
    }
    case SYS_gettid:
    case SYS_getpid:
    case SYS_set_tid_address:
        return 1;
    case SYS_kill:
        if (a1 != 1 && a1 != 0) __builtin_trap(); /* the only process */
        return send_self(a2);
    case SYS_tkill:
        if (a1 != 1) return -ESRCH;
        return send_self(a2);
    case SYS_tgkill:
        if (a1 != 1 || a2 != 1) return -ESRCH;
        return send_self(a3);
    case SYS_exit:
    case SYS_exit_group:
        __bennett_exit((int)a1);
    }
    __builtin_trap();
}
