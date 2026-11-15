/* Substitute for musl's arch/x86_64/atomic_arch.h on a single-threaded device: compare-and-swap is a plain
 * read and write, and a_crash is a trap (the named fault `unreachable`). musl's src/internal/atomic.h derives
 * every other atomic from a_cas; a_ctz/a_clz fall back to its portable C. */
#define a_cas a_cas
static inline int a_cas(volatile int *p, int t, int s)
{
	int old = *p;
	if (old == t) *p = s;
	return old;
}

#define a_cas_p a_cas_p
static inline void *a_cas_p(volatile void *p, void *t, void *s)
{
	void *old = *(void *volatile *)p;
	if (old == t) *(void *volatile *)p = s;
	return old;
}

#define a_crash a_crash
static inline void a_crash()
{
	__builtin_trap();
}
