/* alloca_alignment.c: a local declared _Alignas(64) is at a multiple of 64. Native: 0. */
#include <stdint.h>

__attribute__((noinline)) unsigned low(void *p) { return (uintptr_t)p & 63; }

int main(void) {
  _Alignas(64) int x;
  return low(&x);
}
