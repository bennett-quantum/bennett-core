// Allocation facts must survive native/device compilation without aliasing live objects.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef unsigned long U;
__attribute__((noinline, malloc, alloc_size(1), alloc_align(2)))
void *fresh(size_t bytes, size_t alignment) { return aligned_alloc(alignment, bytes); }
int main(void) {
  for (unsigned seed=0;seed<16;seed++) {
    unsigned char *a = fresh(128, 64), *b = calloc(128, 1);
    if (!a || !b || ((U)a & 63)) return 1;
    for (unsigned i=0;i<128;i++) if (b[i]) return 5;
    for (unsigned i=0;i<128;i++) a[i] = (unsigned char)(i*7+seed);
    memcpy(b, a, 128);
    a = realloc(a, 256);
    if (!a) return 2;
    U hash = 19;
    for (unsigned i=0;i<128;i++) {
      if (a[i] != b[i]) return 3;
      a[i] ^= 0x5a;
      hash = hash*31 + a[i] + 7*b[i];
    }
    free(b);
    b = malloc(128);
    if (!b) return 4;
    memset(b, seed, 128);
    for (unsigned i=0;i<128;i++) hash = hash*3 + a[i] + b[i];
    printf("%lx\n", hash);
    free(b);
    free(a);
  }
  return 0;
}
