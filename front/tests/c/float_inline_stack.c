/* Three conversion sites: aggregate-return runtime inlining increases the entry
 * frame beyond 64 KiB. This is a named resource fault, not a wrong numeric
 * answer; see the ignored integration test. */
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
