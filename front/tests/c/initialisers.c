/* initialisers.c: globals whose initialisers are wider than eight bytes or are not plain integers: wide
 * integers, x86_fp80, fp128, floats, vectors, bit-fields, nested aggregates with padding. byte_at(i) is
 * byte i of all of them, in the order of `parts`; built with -DNATIVE, the program prints every byte. */
typedef int v4 __attribute__((vector_size(16)));
typedef unsigned char v8 __attribute__((vector_size(8)));

struct padded {
  char c;
  long double x;
  short s;
};
struct nested {
  struct padded p[2];
  __int128 wide;
  float f;
  double d;
};
struct bits {
  unsigned a : 3, b : 13;
  signed c : 7;
};

static volatile long double ld = 1.0L;
static volatile long double lds[3] = {1.5L, -2.0L, 3.25L};
static volatile __float128 quad = 1.0;
static volatile __int128 big = ((__int128)0x0123456789abcdefLL << 64) | 0xfedcba9876543210ULL;
static volatile unsigned _BitInt(200) huge =
    (unsigned _BitInt(200))0x12345678ULL << 192 | (unsigned _BitInt(200))0x9abcdef0fedcba98ULL << 128 |
    (unsigned _BitInt(200))0x7654321011223344ULL << 64 | 0x5566778899aabbccULL;
static volatile _BitInt(17) odd[3] = {1, -1, 65535};
static volatile struct nested n = {{{'a', -0.5L, 7}, {'b', 1e300L, -7}}, -3, 2.5f, -0.125};
static volatile v4 vec = {1, -2, 3, -4};
static volatile v8 bytes = {1, 2, 3, 4, 5, 6, 7, 8};
static volatile _Bool flags[3] = {1, 0, 1};
static volatile struct bits bf = {5, 1000, -3};
static volatile _Float16 half = 1.5;

#define PART(x) {(const volatile unsigned char *)&(x), sizeof(x)}
static const struct {
  const volatile unsigned char *bytes;
  unsigned size;
} parts[] = {PART(ld),  PART(lds),   PART(quad),  PART(big),   PART(huge), PART(odd),
             PART(n),   PART(vec),   PART(bytes), PART(flags), PART(bf),   PART(half)};

/* Byte i of the parts in order, or 256 past the end. */
int byte_at(unsigned i) {
  for (unsigned k = 0; k < sizeof parts / sizeof parts[0]; k++) {
    if (i < parts[k].size) return parts[k].bytes[i];
    i -= parts[k].size;
  }
  return 256;
}

#ifdef NATIVE
#include <stdio.h>
int main(void) {
  for (unsigned i = 0; byte_at(i) != 256; i++) printf("%d\n", byte_at(i));
  return 0;
}
#endif
