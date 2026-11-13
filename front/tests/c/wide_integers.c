/* wide_integers.c: integers wider than 64 bits. __int128 arithmetic of every kind, conversions, bit-fields
 * whose storage is wider than 64 bits, 64-bit overflow checks (which are computed in 128 bits), __int128
 * through memory, calls and variadic calls, and a switch on one. Every result is folded into a 64-bit
 * hash, which must equal the hash of the native build (EXPECTED, taken from it with -DPRINT). Returns 0,
 * or 1 if the hash differs. */
#include <stdarg.h>
#ifdef PRINT
#include <stdio.h>
#endif
#define EXPECTED 16166386475620890258ULL

typedef unsigned long long u64;
typedef __int128 i128;
typedef unsigned __int128 u128;

static u64 h = 1469598103934665603ULL;
static void mix(u64 v) { h = (h ^ v) * 1099511628211ULL; }
static void mix128(u128 v) {
  mix((u64)v);
  mix((u64)(v >> 64));
}

static const long long edges[] = {0, 1, -1, 2, -2, 3, 7, -7, 1000003, -1000003, 0x7fffffff, -0x7fffffff - 1,
                                  0xffffffffLL, 0x123456789abcdefLL, -0x123456789abcdefLL,
                                  0x7fffffffffffffffLL, -0x7fffffffffffffffLL - 1, 3037000500LL};
#define EDGES (int)(sizeof edges / sizeof edges[0])

/* A value with both halves set: shifted, negated, combined. */
__attribute__((noinline)) i128 spread(long long a, long long b) { return (i128)(((u128)(u64)a << 64) ^ (u64)b); }

__attribute__((noinline)) void arithmetic(void) {
  for (int i = 0; i < EDGES; i++) {
    for (int j = 0; j < EDGES; j++) {
      i128 a = spread(edges[i], edges[j]), b = (i128)edges[j] * 77 - edges[i];
      u128 ua = (u128)a, ub = (u128)b;
      mix128(ua + ub);
      mix128(ua - ub);
      mix128(ua * ub);
      mix128((u128)((i128)edges[i] * (i128)edges[j])); /* the exact 128-bit product */
      if (b != 0 && !(a == (i128)((u128)1 << 127) && b == -1)) {
        mix128((u128)(a / b));
        mix128((u128)(a % b));
      }
      if (ub != 0) {
        mix128(ua / ub);
        mix128(ua % ub);
      }
      unsigned s = (unsigned)edges[j] & 127;
      mix128(ua << s);
      mix128(ua >> s);
      mix128((u128)(a >> s));
      mix((a < b) | (a <= b) << 1 | (a > b) << 2 | (ua < ub) << 3 | (ua >= ub) << 4 | (a == b) << 5);
      mix128(ua & ub);
      mix128(ua | ~ub);
      /* Conversions: truncation and both extensions. */
      mix((u64)(int)a);
      mix128((u128)(i128)(int)edges[i]);
      mix128((u128)(unsigned)edges[i]);
      mix128((u128)(i128)(signed char)edges[j]);
    }
  }
}

/* 64-bit overflow checks: LLVM's overflow intrinsics at i64, computed in i128. */
__attribute__((noinline)) void overflow(void) {
  for (int i = 0; i < EDGES; i++) {
    for (int j = 0; j < EDGES; j++) {
      long long a = edges[i], b = edges[j], r;
      u64 ua = (u64)a, ub = (u64)b, ur;
      mix(__builtin_mul_overflow(a, b, &r) * 7 + (u64)r);
      mix(__builtin_add_overflow(a, b, &r) * 7 + (u64)r);
      mix(__builtin_sub_overflow(a, b, &r) * 7 + (u64)r);
      mix(__builtin_mul_overflow(ua, ub, &ur) * 7 + ur);
      mix(__builtin_add_overflow(ua, ub, &ur) * 7 + ur);
      mix(__builtin_sub_overflow(ua, ub, &ur) * 7 + ur);
      mix((u64)__builtin_elementwise_add_sat(a, b));
      mix((u64)__builtin_elementwise_sub_sat(a, b));
      mix(__builtin_elementwise_add_sat(ua, ub));
      mix(__builtin_elementwise_sub_sat(ua, ub));
      /* calloc's check */
      mix(b != 0 && ua > ~0ULL / ub);
    }
  }
}

/* Bit-fields whose storage unit is wider than 64 bits. */
struct fields {
  u64 a : 40;
  u64 b : 40;
  long long c : 33;
  unsigned d : 7;
} __attribute__((packed));

__attribute__((noinline)) void bitfields(void) {
  struct fields f = {0};
  for (int i = 0; i < EDGES; i++) {
    f.a = (u64)edges[i];
    f.b = (u64)edges[(i + 3) % EDGES] * 3;
    f.c = edges[(i + 5) % EDGES];
    f.d = (unsigned)i * 37;
    mix(f.a);
    mix(f.b);
    mix((u64)f.c);
    mix(f.d);
    f.a += f.b;
    f.c -= f.a;
    mix(f.a ^ (u64)f.c);
  }
}

__attribute__((noinline)) u128 sum_wide(int n, ...) {
  va_list ap;
  va_start(ap, n);
  u128 s = 0;
  for (int i = 0; i < n; i++) s = s * 3 + va_arg(ap, u128);
  va_end(ap);
  return s;
}

/* Longs, an __int128 and a long after the named argument. With six longs, the registers are full: the
 * sixth long is on the stack, then the __int128 at the next 16-byte boundary. (With four, the registers
 * run out inside the __int128, where LLVM's backend and the psABI disagree; regressions.rs checks the
 * psABI's answer on the device.) */
__attribute__((noinline)) u128 longs_then_wide(int n, ...) {
  va_list ap;
  va_start(ap, n);
  u128 s = 0;
  for (int i = 0; i < n; i++) s = s * 10 + (u64)va_arg(ap, long);
  s = s * 1000 + va_arg(ap, u128);
  s = s * 10 + (u64)va_arg(ap, long);
  va_end(ap);
  return s;
}

__attribute__((noinline)) int classify(i128 v) {
  switch (v) {
  case 0: return 1;
  case -1: return 2;
  case (i128)1 << 100: return 3;
  case ((i128)7 << 64) + 5: return 4;
  default: return 5;
  }
}

static u128 table[4];

int main(void) {
  arithmetic();
  overflow();
  bitfields();
  u128 big = ((u128)0x0123456789abcdefULL << 64) | 0xfedcba9876543210ULL;
  /* After enough integers to use the registers, an __int128 goes on the stack, 16-byte aligned. */
  mix128(sum_wide(4, big, (u128)1 << 127, (u128)-5, big >> 3));
  mix128(sum_wide(1, big));
  mix128(longs_then_wide(6, 1L, 2L, 3L, 4L, 5L, 6L, big, 7L));
  for (int i = 0; i < 4; i++) table[i] = big * (u128)(i + 1);
  for (int i = 0; i < 4; i++) mix128(table[3 - i]);
  i128 cases[] = {0, -1, (i128)1 << 100, ((i128)7 << 64) + 5, ((i128)7 << 64) + 6, 5};
  for (int i = 0; i < 6; i++) mix((u64)classify(cases[i]));
#ifdef PRINT
  printf("%lluULL\n", h);
  return 0;
#else
  return h == EXPECTED ? 0 : 1;
#endif
}
