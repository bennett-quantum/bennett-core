/* vectors.c: GCC's vector extensions, which the front splits into their elements. Arithmetic, shifts and
 * comparisons at several element widths, elements read and written at run-time indices, vectors passed and
 * returned by value and kept in structs, shuffles, conversions, and reinterpretation between vector types
 * and integers. Every result is folded into a 64-bit hash, which must equal the hash of the native build
 * (EXPECTED, taken from it with -DPRINT). Returns 0, or 1 if the hash differs. */
#ifdef PRINT
#include <stdio.h>
#endif
#define EXPECTED 16845414771982879067ULL

typedef unsigned long long u64;
typedef int v4si __attribute__((vector_size(16)));
typedef unsigned v4su __attribute__((vector_size(16)));
typedef short v8hi __attribute__((vector_size(16)));
typedef signed char v16qi __attribute__((vector_size(16)));
typedef unsigned char v8qu __attribute__((vector_size(8)));
typedef long long v2di __attribute__((vector_size(16)));
typedef int v2si __attribute__((vector_size(8)));

static u64 h = 1469598103934665603ULL;
static void mix(u64 v) { h = (h ^ v) * 1099511628211ULL; }
static void mix4(v4si v) {
  for (int i = 0; i < 4; i++) mix((unsigned)v[i]);
}

volatile int seed = 3;

__attribute__((noinline)) v4si combine(v4si a, v4si b, int k) {
  v4si r = (a + b) * k - (a ^ b);
  r += (a < b) & 7;
  r |= (b >> 2) & (v4si){1, 2, 4, 8};
  return r;
}

__attribute__((noinline)) v2di wide(v2di a, v2di b) { return a * b + (a >> 3) - (b << 5); }

struct holder {
  char tag;
  v4si v;
  v2si small;
};

__attribute__((noinline)) struct holder pass_through(struct holder x) {
  x.v = x.v * 3 + x.tag;
  x.small = x.small - 1;
  return x;
}

int main(void) {
  int s = seed;
  v4si a = {s, -s * 7, 1000 * s, -2147483647 - 1};
  v4si b = {5, s + 1, -9, 0x12345678};
  mix4(combine(a, b, s));
  mix4(combine(b, a, -s));
  v4su ua = (v4su)a, ub = (v4su)b;
  v4su q = ua / (ub | 1) + ua % (ub | 1);
  mix4((v4si)q);
  mix4((v4si)(ua >> 3) ^ (v4si)(ub << 1));
  v8hi c = {s, -s, 300 * s, -32768, 32767, 7, -7, 0};
  v8hi d = c * c - (c >> 1);
  for (int i = 0; i < 8; i++) mix((unsigned short)d[i]);
  v16qi e;
  for (int i = 0; i < 16; i++) e[i] = (signed char)(i * s * 37);
  e = e + (e >> 2) - (e < 0);
  for (int i = 0; i < 16; i++) mix((unsigned char)e[(i * 5 + s) & 15]);
  v8qu f = {1, 2, 3, 4, 250, 251, 252, 253};
  f = f * (unsigned char)s + 9;
  for (int i = 0; i < 8; i++) mix(f[i]);
  mix4((v4si)wide((v2di){s, -s}, (v2di){(long long)s << 40, 77}));
  /* Elements written at a run-time index. */
  v4si g = a;
  g[s & 3] = 42;
  g[(s + 1) & 3] += g[s & 3];
  mix4(g);
  /* Reinterpretation between vector types, and with integers. */
  v8hi as_short = (v8hi)a;
  for (int i = 0; i < 8; i++) mix((unsigned short)as_short[i]);
  v2si pair = {s, -s};
  long long whole = (long long)pair;
  mix((u64)whole);
  v2si back = (v2si)(whole + 1);
  mix((unsigned)back[0] ^ (u64)(unsigned)back[1] << 32);
  /* Shuffles and conversions. */
  v4si shuffled = __builtin_shufflevector(a, b, 7, 0, 5, 2);
  mix4(shuffled);
  v4si from_short = __builtin_convertvector(__builtin_shufflevector(c, c, 0, 2, 4, 6), v4si);
  mix4(from_short);
  struct holder x = {(char)s, a, {s, 2 * s}};
  struct holder y = pass_through(x);
  mix4(y.v);
  mix((unsigned)y.small[0] ^ (u64)(unsigned)y.small[1] << 32);
#ifdef PRINT
  printf("%lluULL\n", h);
  return 0;
#else
  return h == EXPECTED ? 0 : 1;
#endif
}
