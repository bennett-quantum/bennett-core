/* Silent wrong result in bennett-front at b875989: a constructor that LLVM cannot evaluate at compile time is
 * never run. Native: exit status 5. Device: `result: 0`. The front lowers @llvm.global_ctors as plain data and
 * nothing calls its entries (the same for @llvm.global_dtors). Expected: run them, or refuse by name. */
volatile int v = 5;
int x;
__attribute__((constructor)) static void init(void) { x = v; }
int main(void) { return x; }
