/* bennett-front segfaults in LLVM 18.1.3 DeadStoreElimination (malloc + memset -> calloc fold) when the
 * module defines calloc and a non-inlinable malloc. Stock `opt-18 -passes=default<O2>` crashes the same way.
 * Expected: a device program, or a refusal by name. Observed: exit 139, stack in llvm::DSEPass::run. */
typedef __SIZE_TYPE__ size_t;
void *memset(void *, int, size_t);
static char heap[256];
size_t used;
__attribute__((noinline)) void *malloc(size_t n) { used = 0; return heap + (n & 1); }
void *calloc(size_t n, size_t s) { return 0; }
int main(void) { char *p = malloc(94); memset(p, 0, 94); return 0; }
