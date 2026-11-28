#ifndef BENNETT_ARGUMENTS_H
#define BENNETT_ARGUMENTS_H
/* Each system call's scalar arguments as Linux takes them (rt/musl/README.md, "Arguments"). The kernel's entry
 * (SYSCALL_DEFINE) converts each 64-bit argument to the type the call declares before the call looks at it, so
 * the upper bits of a narrower argument never matter: an int keeps its low 32 bits, sign-extended; an unsigned
 * int its low 32 bits; a mode (umode_t) its low 16 bits. A pointer, a size, a long or an offset is whole. The
 * runtime converts every argument so at its one entry, before any call looks at one. */
#include <sys/syscall.h>

enum { BENNETT_WHOLE, BENNETT_INT, BENNETT_UNSIGNED, BENNETT_MODE };

/* The kinds of a call's first four arguments, two bits each from the first; the rest are whole. */
#define BENNETT_KINDS(a, b, c, d) ((a) | (b) << 2 | (c) << 4 | (d) << 6)

/* The declared types of the arguments of each call the runtime implements, from the kernel's
 * include/linux/syscalls.h (x86-64). readv and writev declare the descriptor and the count unsigned long and
 * pass both on as unsigned int (fdget_pos, import_iovec), so they are taken as unsigned int. A call that is
 * absent has only whole arguments (mmap's are all unsigned long on x86-64). */
static inline unsigned linux_argument_kinds(long number)
{
    enum { W = BENNETT_WHOLE, I = BENNETT_INT, U = BENNETT_UNSIGNED, M = BENNETT_MODE };
    switch (number) {
    case SYS_read: case SYS_write: case SYS_pread64: case SYS_pwrite64: case SYS_fstat: case SYS_ftruncate:
    case SYS_close: case SYS_dup: case SYS_fsync: case SYS_fdatasync: case SYS_fchdir:
        return BENNETT_KINDS(U, W, W, W);
    case SYS_readv: case SYS_writev: return BENNETT_KINDS(U, W, U, W);
    case SYS_lseek: return BENNETT_KINDS(U, W, U, W);
    case SYS_ioctl: case SYS_fcntl: case SYS_dup2: return BENNETT_KINDS(U, U, W, W);
    case SYS_dup3: return BENNETT_KINDS(U, U, I, W);
    case SYS_open: return BENNETT_KINDS(W, I, M, W);
    case SYS_openat: return BENNETT_KINDS(I, W, I, M);
    case SYS_creat: case SYS_mkdir: case SYS_chmod: return BENNETT_KINDS(W, M, W, W);
    case SYS_mkdirat: return BENNETT_KINDS(I, W, M, W);
    case SYS_access: return BENNETT_KINDS(W, I, W, W);
    case SYS_faccessat: case SYS_unlinkat: return BENNETT_KINDS(I, W, I, W);
    case SYS_readlink: return BENNETT_KINDS(W, W, I, W);
    case SYS_readlinkat: return BENNETT_KINDS(I, W, W, I);
    case SYS_newfstatat: return BENNETT_KINDS(I, W, W, I);
    case SYS_umask: case SYS_exit: case SYS_exit_group: return BENNETT_KINDS(I, W, W, W);
    case SYS_clock_gettime: case SYS_clock_getres: return BENNETT_KINDS(I, W, W, W);
    case SYS_clock_nanosleep: return BENNETT_KINDS(I, I, W, W);
    case SYS_getrandom: return BENNETT_KINDS(W, W, U, W);
    case SYS_madvise: return BENNETT_KINDS(W, W, I, W);
    case SYS_rt_sigprocmask: case SYS_rt_sigaction: return BENNETT_KINDS(I, W, W, W);
    case SYS_kill: case SYS_tkill: return BENNETT_KINDS(I, I, W, W);
    case SYS_tgkill: return BENNETT_KINDS(I, I, I, W);
    }
    return 0;
}

/* Converts the six arguments of call `number` in place to the types the call declares. */
static inline void __bennett_linux_arguments(long number, long argument[6])
{
    unsigned kinds = linux_argument_kinds(number);
    for (int i = 0; kinds; ++i, kinds >>= 2) {
        switch (kinds & 3) {
        case BENNETT_WHOLE: break;
        case BENNETT_INT: argument[i] = (int)argument[i]; break;
        case BENNETT_UNSIGNED: argument[i] = (unsigned)argument[i]; break;
        case BENNETT_MODE: argument[i] = (unsigned short)argument[i]; break;
        }
    }
}
#endif
