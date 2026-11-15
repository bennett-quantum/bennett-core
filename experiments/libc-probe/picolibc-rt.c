/* The probe's boundary under picolibc: its POSIX porting layer (write, read, _exit, sbrk and a few stubs) and the
 * startup a C runtime does, in C. Compiled with picolibc's own flags (picolibc-bc.sh writes them to <out>/cflags)
 * and linked with the program before the library. Not a product.
 *
 * The oracle is the one of musl-rt.c: bytes written to fd 1 go to out_buf; a return from main makes the device
 * result a 64-bit FNV-1a hash of (those bytes, exit status mod 256); a run that ends in _exit (exit, abort)
 * compares that hash with BENNETT_EXPECT and ends in a fault that says whether it matched (variant A: match ->
 * bad_indirect, mismatch -> divide_by_zero; -DRT_B the other way round).
 *
 * Opening a file traps (no file system, the named fault `unreachable`) unless FILES_ENOENT is defined.
 * BENNETT_EFFECT: also write each byte for fd 1 to the device console (needs the front of front-effect.patch).
 * MAIN_ARGS (0, 2 or 3): the parameters of the program's main.
 */
#include <errno.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#ifndef MAIN_ARGS
#define MAIN_ARGS 0
#endif
#ifndef BENNETT_EXPECT
#define BENNETT_EXPECT 0
#endif
#ifndef OUT_CAP
#define OUT_CAP (64 << 10)
#endif
#ifndef ARENA
#define ARENA (1 << 20)
#endif

#ifdef BENNETT_EFFECT
/* The device's console, one byte (front-effect.patch); only with the experimental front. */
void __bennett_effect(char);
#endif

static unsigned char out_buf[OUT_CAP];
static size_t out_len;
static unsigned char arena[ARENA] __attribute__((aligned(16)));
static size_t arena_used;

static volatile int rt_zero = 0;
static volatile int rt_one = 1;
volatile int rt_sink;
static void (*volatile rt_bad_target)(void) = (void (*)(void))(size_t)0x7e57;

static void __attribute__((noinline, noreturn)) rt_divide_by_zero(void)
{
    for (;;)
        rt_sink = rt_one / rt_zero;
}

static void __attribute__((noinline, noreturn)) rt_bad_indirect(void)
{
    for (;;)
        rt_bad_target();
}

#ifdef RT_B
#define RT_MATCH    rt_divide_by_zero
#define RT_MISMATCH rt_bad_indirect
#else
#define RT_MATCH    rt_bad_indirect
#define RT_MISMATCH rt_divide_by_zero
#endif

static unsigned long long hash(int status)
{
    unsigned long long h = 0xcbf29ce484222325ull;
    for (size_t i = 0; i < out_len; i++) {
        h ^= out_buf[i];
        h *= 0x100000001b3ull;
    }
    h ^= (unsigned char)status;
    h *= 0x100000001b3ull;
    return h;
}

void _exit(int status)
{
    if (hash(status) == BENNETT_EXPECT)
        RT_MATCH();
    RT_MISMATCH();
}

ssize_t write(int fd, const void *buf, size_t n)
{
    if (fd == 2)
        return n; /* stderr is not compared */
    if (fd != 1) {
        errno = EBADF;
        return -1;
    }
    if (n > OUT_CAP - out_len)
        __builtin_trap();
    for (size_t i = 0; i < n; i++) {
        out_buf[out_len + i] = ((const unsigned char *)buf)[i];
#ifdef BENNETT_EFFECT
        __bennett_effect(((const char *)buf)[i]);
#endif
    }
    out_len += n;
    return n;
}

ssize_t read(int fd, void *buf, size_t n)
{
    (void)buf;
    (void)n;
    if (fd == 0)
        return 0; /* stdin is empty, as with </dev/null */
    errno = EBADF;
    return -1;
}

void *sbrk(ptrdiff_t incr)
{
    if (incr < 0 ? (size_t)-incr > arena_used : (size_t)incr > ARENA - arena_used) {
        errno = ENOMEM;
        return (void *)-1;
    }
    void *old = arena + arena_used;
    arena_used += incr;
    return old;
}

int open(const char *path, int flags, ...)
{
    (void)path;
    (void)flags;
#ifdef FILES_ENOENT
    errno = ENOENT;
    return -1;
#else
    __builtin_trap(); /* no file system */
#endif
}

int unlink(const char *path)
{
    (void)path;
#ifdef FILES_ENOENT
    errno = ENOENT;
    return -1;
#else
    __builtin_trap(); /* no file system */
#endif
}

int close(int fd)
{
    (void)fd;
    return 0;
}

off_t lseek(int fd, off_t offset, int whence)
{
    (void)fd;
    (void)offset;
    (void)whence;
    errno = ESPIPE;
    return -1;
}

int fstat(int fd, struct stat *st)
{
    (void)fd;
    (void)st;
    errno = ENOSYS;
    return -1;
}

int isatty(int fd)
{
    (void)fd;
    return 0;
}

pid_t getpid(void) { return 1; }

int kill(pid_t pid, int sig)
{
    (void)pid;
    _exit(128 + sig);
}

/* Constructors and destructors, as ctors.py lays them out (a static linker would fill .init_array and
 * .fini_array, which picolibc's __libc_init_array and __libc_fini_array walk). picolibc's exit() calls
 * __libc_fini_array and then _exit; this strong definition replaces picolibc's. */
extern void (*const __bennett_ctors[])(void);
extern const long __bennett_ctors_n;
extern void (*const __bennett_dtors[])(void);
extern const long __bennett_dtors_n;

void __libc_init_array(void)
{
    for (long i = 0; i < __bennett_ctors_n; i++)
        __bennett_ctors[i]();
}

void __libc_fini_array(void)
{
    for (long i = __bennett_dtors_n; i > 0; i--)
        __bennett_dtors[i - 1]();
}

static char  progname[] = "prog";
static char *argv[2] = { progname, 0 };
static char *envp[1];

#if MAIN_ARGS == 0
int main(void);
#define CALL_MAIN main()
#elif MAIN_ARGS == 2
int main(int, char **);
#define CALL_MAIN main(1, argv)
#else
int main(int, char **, char **);
#define CALL_MAIN main(1, argv, envp)
#endif

unsigned long long __bennett_entry(void)
{
    __libc_init_array();
    int status = CALL_MAIN;
    __libc_fini_array();
    return hash(status);
}
