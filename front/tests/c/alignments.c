/* alignments.c: every object is at a multiple of its alignment: a local, an over-aligned struct type in an
 * array, a dynamic allocation with an alignment, and globals. Native: 0. */
#include <stdint.h>

struct wide {
  _Alignas(128) char c;
};

_Alignas(4096) static volatile char page[3] = {1, 2, 3};
static volatile struct wide globals[2];
static volatile unsigned count = 1;

__attribute__((noinline)) unsigned misaligned(const volatile void *p, uintptr_t align) {
  return (uintptr_t)p & (align - 1);
}

int main(void) {
  _Alignas(64) volatile int x = 0;
  volatile struct wide locals[2];
  char *dynamic = __builtin_alloca_with_align(count * 24, 256 * 8);
  dynamic[0] = 1;
  unsigned bad = misaligned(&x, 64) | misaligned(page, 4096) | misaligned(&globals[1], 128) |
                 misaligned(&locals[1], 128) | misaligned(dynamic, 256);
  return (int)bad + x + page[2] - 3 + dynamic[0] - 1;
}
