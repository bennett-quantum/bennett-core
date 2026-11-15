/* Substitute for musl's arch/x86_64/pthread_arch.h: the thread pointer is the address of one static
 * thread descriptor, set up by the probe runtime (musl-rt.c) instead of musl's __init_tls. */
struct __pthread; /* musl renames struct pthread to struct __pthread (src/internal/pthread_impl.h) */
extern struct __pthread __bennett_thread;
static inline uintptr_t __get_tp()
{
	return (uintptr_t)&__bennett_thread;
}

#define MC_PC gregs[REG_RIP]
