/* Three conversion sites within a 64 KiB device stack: inlining the runtime's
 * aggregate-returning routines at every site once overflowed it. */
typedef unsigned long long u64;
volatile long double x = 0x1.23456789abcdefp64L;
u64 result(int i) {
  __int128 s = 0;
  s += (__int128)(x + (long double)(i + 0));
  s += (__int128)(x + (long double)(i + 1));
  s += (__int128)(x + (long double)(i + 2));
  return (u64)s ^ (u64)((unsigned __int128)s >> 64);
}
int results(void) { return 8; }
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  for (int i = 0; i < 8; i++)
    printf("%016llx\n", result(i));
}
#endif
