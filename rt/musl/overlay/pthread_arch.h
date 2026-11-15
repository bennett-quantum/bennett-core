/* Substitute for musl's arch/x86_64/pthread_arch.h: the thread pointer is the pointer installed by musl's own
 * TLS initialization through Bennett's __set_thread_area boundary. */
struct __pthread; /* musl renames struct pthread to struct __pthread (src/internal/pthread_impl.h) */
extern struct __pthread *__bennett_thread_pointer;
static inline uintptr_t __get_tp()
{
	return (uintptr_t)__bennett_thread_pointer;
}

#define MC_PC gregs[REG_RIP]
