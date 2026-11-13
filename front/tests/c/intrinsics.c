/* intrinsics.c: builtins that clang turns into LLVM intrinsics, on many inputs. Every result is folded
 * into one 64-bit hash, which must equal the hash the native build computes (EXPECTED, taken from it
 * with -DPRINT). Comparing with C written without the builtins would not do: LLVM may turn that C into
 * the same intrinsic, and a wrong lowering would then agree with itself. Returns 0, or 1 if the hash
 * differs, or 2 if a check that needs no hash fails. */
#ifdef PRINT
#include <stdio.h>
#endif
#define EXPECTED 15373840836155232888ULL

typedef unsigned long long u64;

static u64 h = 1469598103934665603ULL;
static void mix(u64 v) { h = (h ^ v) * 1099511628211ULL; }

/* Inputs: edge values of each width and a spread between them. */
static const long long edges[] = {0, 1, 2, 3, -1, -2, 7, -7, 100, -100, 127, -128, 128, 255, 256, -129,
                                  32767, -32768, 65535, 46341, -46341, 0x7fffffff, -0x7fffffff - 1,
                                  0x12345678, -0x12345678, 0xffffffff, 0x100000000LL, 0x7fffffffffffffffLL,
                                  -0x7fffffffffffffffLL - 1, 0x0123456789abcdefLL, -3037000500LL, 3037000500LL};
#define EDGES (int)(sizeof edges / sizeof edges[0])

__attribute__((noinline)) void bytes(void) {
  for (int i = -128; i < 128; i += 3) {
    for (int j = -128; j < 128; j += 5) {
      signed char a = (signed char)i, b = (signed char)j, r;
      mix(__builtin_add_overflow(a, b, &r) * 1000 + (unsigned char)r);
      mix(__builtin_sub_overflow(a, b, &r) * 1000 + (unsigned char)r);
      mix(__builtin_mul_overflow(a, b, &r) * 1000 + (unsigned char)r);
      unsigned char ua = (unsigned char)i, ub = (unsigned char)j, ur;
      mix(__builtin_add_overflow(ua, ub, &ur) * 1000 + ur);
      mix(__builtin_sub_overflow(ua, ub, &ur) * 1000 + ur);
      mix(__builtin_mul_overflow(ua, ub, &ur) * 1000 + ur);
      mix((unsigned char)__builtin_elementwise_add_sat(a, b));
      mix((unsigned char)__builtin_elementwise_sub_sat(a, b));
      mix(__builtin_elementwise_add_sat(ua, ub));
      mix(__builtin_elementwise_sub_sat(ua, ub));
    }
  }
}

__attribute__((noinline)) void words(void) {
  for (int i = 0; i < EDGES; i++) {
    for (int j = 0; j < EDGES; j++) {
      short a16 = (short)edges[i], b16 = (short)edges[j], r16;
      mix(__builtin_mul_overflow(a16, b16, &r16) * 100000 + (unsigned short)r16);
      unsigned short ua16 = (unsigned short)edges[i], ub16 = (unsigned short)edges[j], ur16;
      mix(__builtin_add_overflow(ua16, ub16, &ur16) * 100000 + ur16);
      int a = (int)edges[i], b = (int)edges[j], r;
      mix(__builtin_add_overflow(a, b, &r) * 10000000000ULL + (unsigned)r);
      mix(__builtin_sub_overflow(a, b, &r) * 10000000000ULL + (unsigned)r);
      mix(__builtin_mul_overflow(a, b, &r) * 10000000000ULL + (unsigned)r);
      unsigned ua = (unsigned)edges[i], ub = (unsigned)edges[j], ur;
      mix(__builtin_add_overflow(ua, ub, &ur) * 10000000000ULL + ur);
      mix(__builtin_sub_overflow(ua, ub, &ur) * 10000000000ULL + ur);
      mix(__builtin_mul_overflow(ua, ub, &ur) * 10000000000ULL + ur);
      mix((unsigned)__builtin_elementwise_add_sat(a, b));
      mix((unsigned)__builtin_elementwise_sub_sat(a, b));
      mix(__builtin_elementwise_add_sat(ua, ub));
      mix(__builtin_elementwise_sub_sat(ua, ub));
    }
  }
}

__attribute__((noinline)) void bits(void) {
  for (int i = 0; i < EDGES; i++) {
    u64 x = (u64)edges[i];
    mix(__builtin_bitreverse8((unsigned char)x));
    mix(__builtin_bitreverse16((unsigned short)x));
    mix(__builtin_bitreverse32((unsigned)x));
    mix(__builtin_bitreverse64(x));
    mix((u64)__builtin_popcountll(x) << 8 | (unsigned)__builtin_popcount((unsigned)x));
    if (x) mix((u64)__builtin_clzll(x) << 8 | (unsigned)__builtin_ctzll(x));
    if ((unsigned)x) mix((u64)__builtin_clz((unsigned)x) << 8 | (unsigned)__builtin_ctz((unsigned)x));
    mix(__builtin_bswap16((unsigned short)x));
    mix(__builtin_bswap32((unsigned)x));
    mix(__builtin_bswap64(x));
    if ((int)x != -0x7fffffff - 1) mix((unsigned)__builtin_abs((int)x)); /* abs(INT_MIN) is undefined */
    if ((long long)x != -0x7fffffffffffffffLL - 1) mix((u64)__builtin_llabs((long long)x));
    long long y = edges[(i * 7 + 3) % EDGES];
    mix((u64)((long long)x < y ? (long long)x : y));
    mix(x > (u64)y ? x : (u64)y);
    mix(__builtin_rotateleft32((unsigned)x, (unsigned)y));
    mix(__builtin_rotateright64(x, (u64)y));
  }
}

/* A variable-length array in a loop: clang saves and restores the stack around it. */
__attribute__((noinline)) long arrays(int n) {
  long total = 0;
  for (int i = 1; i <= n; i++) {
    long a[i];
    for (int j = 0; j < i; j++) a[j] = j * i;
    __builtin_prefetch(&a[i - 1]);
    total += a[i - 1];
  }
  return total;
}

volatile int seven = 7;

int main(void) {
  bytes();
  words();
  bits();
  mix((u64)arrays(seven * 3));
  char buffer[24];
  int n = seven;
  if (__builtin_expect(n == 7, 1)) mix(11);
  __builtin_assume(n > 0);
  mix(__builtin_object_size(buffer, 0));
  mix(__builtin_object_size(buffer + n, 0));
  mix(__builtin_constant_p(n) * 2 + __builtin_constant_p(5));
#ifdef PRINT
  printf("%lluULL\n", h);
  return 0;
#else
  return h == EXPECTED ? 0 : 1;
#endif
}
