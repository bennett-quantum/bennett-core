/* The probe's boundary under musl: startup, the thread descriptor, and every system call musl makes, in C.
 * Compiled with musl's own flags and internal headers (musl-bc.sh writes them to <out>/cflags), linked with the
 * program before the library. Not a product: the smallest thing that lets musl run on a device that has no
 * operating system, no console instruction reachable from C, and ends a run only by returning or by a fault.
 *
 * Output. A write to fd 1 appends to out_buf. The result of a run is a 64-bit FNV-1a hash of the bytes written to
 * fd 1 followed by one byte, the exit status mod 256. Native runs compute the same hash of their real stdout.
 *   - main returns r: __bennett_entry runs what musl's exit() runs before _Exit (atexit handlers, .fini, the stdio
 *     flush) and returns hash(out, r) as the device result.
 *   - the program calls exit(r), or abort() (status 134, as a shell sees SIGABRT), or the run otherwise reaches
 *     exit_group: the run cannot return from here, so the hash is compared with BENNETT_EXPECT (the native hash,
 *     given at compile time) and the run ends in a fault that says which: variant A (default) match ->
 *     bad_indirect, mismatch -> divide_by_zero; variant B (-DRT_B) the other way round. The harness reruns with B
 *     and needs the fault kind to swap, as in experiments/c-corpus/rt.c.
 * Opening a file traps too (no file system), unless FILES_ENOENT is defined: then it fails with ENOENT.
 * Any system call not listed below traps (the named fault `unreachable`), so nothing unplanned passes silently.
 *
 * BENNETT_DEVICE_EXIT: use the real device exit and console, return an int status, and use standard linker
 * array bounds (the front supplies them). The default hash-and-fault probe is unchanged.
 * BENNETT_EFFECT: also write each byte for fd 1 to the device console.
 * MAIN_ARGS (0, 2 or 3): the parameters of the program's main, so that the call matches its prototype.
 */
#include "libc.h"
#include "pthread_impl.h"
#include <errno.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>

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

struct pthread __bennett_thread;

#ifdef BENNETT_DEVICE_EXIT
#ifndef BENNETT_EFFECT
#define BENNETT_EFFECT
#endif
_Noreturn void __bennett_exit(int);
#endif

#ifdef BENNETT_EFFECT
/* The device console, one byte. */
void __bennett_effect(char);
#endif

static unsigned char out_buf[OUT_CAP];
static size_t out_len;
static unsigned char arena[ARENA] __attribute__((aligned(4096)));
static size_t brk_used, mmap_used; /* brk grows up from the bottom, mmap down from the top */

static volatile int rt_zero = 0;
static volatile int rt_one = 1;
volatile int rt_sink;
static void (*volatile rt_bad_target)(void) = (void (*)(void))(size_t)0x7e57;

static void __attribute__((noinline, noreturn)) rt_divide_by_zero(void)
{
	for (;;) rt_sink = rt_one / rt_zero;
}

static void __attribute__((noinline, noreturn)) rt_bad_indirect(void)
{
	for (;;) rt_bad_target();
}

#ifdef RT_B
#define RT_MATCH rt_divide_by_zero
#define RT_MISMATCH rt_bad_indirect
#else
#define RT_MATCH rt_bad_indirect
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

static void __attribute__((noreturn)) end(int status)
{
#ifdef BENNETT_DEVICE_EXIT
	__bennett_exit(status);
#else
	if (hash(status) == BENNETT_EXPECT) RT_MATCH();
	RT_MISMATCH();
#endif
}

static long out_write(int fd, const void *buf, size_t n)
{
	if (fd == 2) return n; /* stderr is not compared */
	if (fd != 1) return -EBADF;
	if (n > OUT_CAP - out_len) __builtin_trap();
	for (size_t i = 0; i < n; i++) {
		out_buf[out_len + i] = ((const unsigned char *)buf)[i];
#ifdef BENNETT_EFFECT
		__bennett_effect(((const char *)buf)[i]);
#endif
	}
	out_len += n;
	return n;
}

long __bennett_syscall(long n, long a1, long a2, long a3, long a4, long a5, long a6)
{
	(void)a5;
	(void)a6;
	switch (n) {
	case SYS_write:
		return out_write(a1, (const void *)a2, a3);
	case SYS_writev: {
		const struct iovec *iov = (const struct iovec *)a2;
		long total = 0;
		for (long i = 0; i < a3; i++) {
			long r = out_write(a1, iov[i].iov_base, iov[i].iov_len);
			if (r < 0) return r;
			total += r;
		}
		return total;
	}
	case SYS_read:
		return a1 == 0 ? 0 : -EBADF;
	case SYS_ioctl:
		return -ENOTTY; /* not a terminal: stdout is fully buffered, as when native output goes to a pipe */
	case SYS_open:
	case SYS_openat:
#ifdef FILES_ENOENT
		return -ENOENT; /* no files: every open fails, as for a missing path */
#else
		__builtin_trap(); /* no file system: the named fault `unreachable`, not a result that differs from native */
#endif
	case SYS_close:
		return 0;
	case SYS_lseek:
		return -ESPIPE;
	case SYS_brk: {
		uintptr_t base = (uintptr_t)arena, want = (uintptr_t)a1;
		if (want >= base + brk_used && want <= base + ARENA - mmap_used) brk_used = want - base;
		return base + brk_used;
	}
	case SYS_mmap: {
		size_t len = ((size_t)a2 + 4095) & ~(size_t)4095;
		if (!(a4 & MAP_ANONYMOUS) || len > ARENA - mmap_used - brk_used) return -ENOMEM;
		mmap_used += len;
		return (long)(arena + ARENA - mmap_used);
	}
	case SYS_munmap:
	case SYS_madvise:
	case SYS_mprotect:
		return 0;
	case SYS_mremap:
		return -ENOMEM;
	case SYS_rt_sigprocmask:
	case SYS_rt_sigaction:
		return 0;
	case SYS_gettid:
	case SYS_getpid:
	case SYS_set_tid_address:
		return 1;
	case SYS_tkill:
	case SYS_tgkill:
	case SYS_kill:
		end(128 + (int)(n == SYS_tgkill ? a3 : a2));
	case SYS_exit:
	case SYS_exit_group:
		end((int)a1);
	}
	__builtin_trap();
}

/* What musl's startup (__init_libc, __init_tls) sets that the library reads. */
extern char **__environ;
extern char *__progname, *__progname_full;

static size_t auxv[2];
static char *envp[1];
static char progname[] = "prog";
static char *argv[2] = {progname, 0};

static void init(void)
{
	struct pthread *td = &__bennett_thread;
	td->self = td;
	td->detach_state = DT_JOINABLE;
	td->tid = 1;
	td->locale = &libc.global_locale;
	td->robust_list.head = &td->robust_list.head;
	td->next = td->prev = td;
	libc.auxv = auxv;
	libc.page_size = 4096;
	__environ = envp;
	__progname = __progname_full = progname;
}

/* Constructors and destructors, as ctors.py lays them out from the linked module (a static linker would put them
 * in .init_array and .fini_array, which musl's __libc_start_init and __libc_exit_fini walk). This strong
 * __libc_exit_fini replaces musl's weak one, so exit() runs the destructors too. */
#ifdef BENNETT_DEVICE_EXIT
extern void (*const __init_array_start[])(void), (*const __init_array_end[])(void);
extern void (*const __fini_array_start[])(void), (*const __fini_array_end[])(void);
#define __bennett_ctors __init_array_start
#define __bennett_ctors_n (__init_array_end - __init_array_start)
#define __bennett_dtors __fini_array_start
#define __bennett_dtors_n (__fini_array_end - __fini_array_start)
#else
extern void (*const __bennett_ctors[])(void);
extern const long __bennett_ctors_n;
extern void (*const __bennett_dtors[])(void);
extern const long __bennett_dtors_n;
#endif

void __libc_exit_fini(void)
{
	for (long i = __bennett_dtors_n; i > 0; i--) __bennett_dtors[i - 1]();
}

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

void __stdio_exit(void);

#ifdef BENNETT_DEVICE_EXIT
int
#else
unsigned long long
#endif
__bennett_entry(void)
{
	init();
	for (long i = 0; i < __bennett_ctors_n; i++) __bennett_ctors[i]();
	int status = CALL_MAIN;
	__funcs_on_exit();
	__libc_exit_fini();
	__stdio_exit();
#ifdef BENNETT_DEVICE_EXIT
	return status;
#else
	return hash(status);
#endif
}
