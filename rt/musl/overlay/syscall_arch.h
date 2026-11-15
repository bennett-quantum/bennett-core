/* Substitute for musl's arch/x86_64/syscall_arch.h: every system call is an ordinary C call of
 * __bennett_syscall, defined by Bennett's runtime. No inline assembly, no vDSO. */
#define __SYSCALL_LL_E(x) (x)
#define __SYSCALL_LL_O(x) (x)

long __bennett_syscall(long n, long a1, long a2, long a3, long a4, long a5, long a6);

static __inline long __syscall0(long n) { return __bennett_syscall(n, 0, 0, 0, 0, 0, 0); }
static __inline long __syscall1(long n, long a1) { return __bennett_syscall(n, a1, 0, 0, 0, 0, 0); }
static __inline long __syscall2(long n, long a1, long a2) { return __bennett_syscall(n, a1, a2, 0, 0, 0, 0); }
static __inline long __syscall3(long n, long a1, long a2, long a3)
{
	return __bennett_syscall(n, a1, a2, a3, 0, 0, 0);
}
static __inline long __syscall4(long n, long a1, long a2, long a3, long a4)
{
	return __bennett_syscall(n, a1, a2, a3, a4, 0, 0);
}
static __inline long __syscall5(long n, long a1, long a2, long a3, long a4, long a5)
{
	return __bennett_syscall(n, a1, a2, a3, a4, a5, 0);
}
static __inline long __syscall6(long n, long a1, long a2, long a3, long a4, long a5, long a6)
{
	return __bennett_syscall(n, a1, a2, a3, a4, a5, a6);
}

#define IPC_64 0
