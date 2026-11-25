/* The floating-point environment against the native program: every operation under each of the four rounding
 * modes, its result's bits and the exception flags it raised, under `#pragma STDC FENV_ACCESS ON`. Compiled
 * natively (glibc) and for the device (musl), run, and the outputs compared line for line by
 * `front/tests/tests/fenv.rs`.
 *
 * Each group prints one line: its name, the number of cases and a hash of every case's result bits and flags.
 * With -DVERBOSE every case is printed instead, to find a difference. -DGROUP=n runs only group n (from 0).
 * With -DRANDOM=n the groups are seeded random operands instead, n per group and mode.
 * Operands come through volatile memory so that nothing is folded. */
#include <fenv.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#pragma STDC FENV_ACCESS ON

#define FLAGS (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)

static const int modes[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};

static uint64_t hash;
static unsigned long cases;
static const char *group;
static int mode_index;

static void begin(const char *name) {
  group = name;
  hash = 0xcbf29ce484222325u;
  cases = 0;
}
static void end(void) {
#ifndef VERBOSE
  printf("%s %lu %016llx\n", group, cases, (unsigned long long)hash);
#endif
}
static void mix(const void *bytes, size_t n) {
  const unsigned char *p = bytes;
  for (size_t i = 0; i < n; ++i) {
    hash ^= p[i];
    hash *= 0x100000001b3u;
  }
}
/* One case: `n` result bytes, then the flags raised since the last clear. */
static void record(unsigned a, unsigned b, const void *result, size_t n) {
  int f = fetestexcept(FLAGS);
  unsigned char flags = (unsigned char)f;
  mix(result, n);
  mix(&flags, 1);
  ++cases;
#ifdef VERBOSE
  printf("%s m%d %u %u ", group, mode_index, a, b);
  for (size_t i = n; i-- > 0;) printf("%02x", ((const unsigned char *)result)[i]);
  printf(" %02x\n", flags);
#else
  (void)a;
  (void)b;
#endif
}
static void start(void) { feclearexcept(FE_ALL_EXCEPT); }

/* Directed operands, as bits. */
static const uint32_t f32_bits[] = {
    0x00000000, 0x80000000, 0x00000001, 0x007fffff, 0x00800000, 0x80800001, 0x3f800000, 0xbf800000,
    0x3f800001, 0x3f7fffff, 0x3fc00000, 0x40200000, 0xc0200000, 0x40400000, 0x3dcccccd, 0x3f000000,
    0xbf000000, 0x4b000001, 0x4effffff, 0x4f000000, 0xcf000000, 0xcf000001, 0x5f000000, 0x5f800000,
    0x7f7fffff, 0xff7fffff, 0x7f800000, 0xff800000, 0x7fc00000, 0x7fa00000, 0xffc12345, 0x1e3ce508,
};
static const uint64_t f64_bits[] = {
    0x0000000000000000, 0x8000000000000000, 0x0000000000000001, 0x000fffffffffffff, 0x0010000000000000,
    0x8010000000000001, 0x3ff0000000000000, 0xbff0000000000000, 0x3ff0000000000001, 0x3fefffffffffffff,
    0x3ff8000000000000, 0x4004000000000000, 0xc004000000000000, 0x4008000000000000, 0x3fb999999999999a,
    0x3fe0000000000000, 0xbfe0000000000000, 0x4330000000000001, 0x41dfffffffc00000, 0x41e0000000000000,
    0xc1e0000000000000, 0xc1e0000000200000, 0x43e0000000000000, 0x43f0000000000000, 0x7fefffffffffffff,
    0xffefffffffffffff, 0x7ff0000000000000, 0xfff0000000000000, 0x7ff8000000000000, 0x7ff4000000000000,
    0xfff8000000012345, 0x36a0000000000001, 0x47efffffe0000000, 0x3810000000000000,
};
/* long double: sign and exponent, significand (x87's explicit integer bit). */
struct x80 {
  uint16_t se;
  uint64_t m;
};
static const struct x80 f80_bits[] = {
    {0x0000, 0}, {0x8000, 0}, {0x0000, 1}, {0x0000, 0x7fffffffffffffff}, {0x0001, 0x8000000000000000},
    {0x8001, 0x8000000000000001}, {0x3fff, 0x8000000000000000}, {0xbfff, 0x8000000000000000},
    {0x3fff, 0x8000000000000001}, {0x3ffe, 0xffffffffffffffff}, {0x3fff, 0xc000000000000000},
    {0x4000, 0xa000000000000000}, {0xc000, 0xa000000000000000}, {0x4000, 0xc000000000000000},
    {0x3ffb, 0xcccccccccccccccd}, {0x3ffe, 0x8000000000000000}, {0xbffe, 0x8000000000000000},
    {0x403e, 0x8000000000000001}, {0x401d, 0xfffffffe00000000}, {0x401e, 0x8000000000000000},
    {0xc01e, 0x8000000000000000}, {0x403e, 0x8000000000000000}, {0x403f, 0x8000000000000000},
    {0x7ffe, 0xffffffffffffffff}, {0xfffe, 0xffffffffffffffff}, {0x7fff, 0x8000000000000000},
    {0xffff, 0x8000000000000000}, {0x7fff, 0xc000000000000000}, {0x7fff, 0xa000000000000000},
    {0xffff, 0xc000000000012345}, {0x3c00, 0x8000000000000001}, {0x43fe, 0xffffffffffffffff},
    {0x3bcd, 0xf000000000000000}, {0x3fff, 0x4000000000000000}, {0x0000, 0x8000000000000001},
};
#define COUNT(a) (sizeof(a) / sizeof(a)[0])

static volatile _Float16 vh[COUNT(f32_bits)];
static volatile float vf[COUNT(f32_bits)];
static volatile double vd[COUNT(f64_bits)];
static volatile long double vl[COUNT(f80_bits)];
static void load(void) {
  for (unsigned i = 0; i < COUNT(f32_bits); ++i) {
    float x;
    memcpy(&x, &f32_bits[i], 4);
    vf[i] = x;
    vh[i] = (_Float16)x;
  }
  for (unsigned i = 0; i < COUNT(f64_bits); ++i) {
    double x;
    memcpy(&x, &f64_bits[i], 8);
    vd[i] = x;
  }
  for (unsigned i = 0; i < COUNT(f80_bits); ++i) {
    long double x = 0;
    memcpy(&x, &f80_bits[i].m, 8);
    memcpy((char *)&x + 8, &f80_bits[i].se, 2);
    vl[i] = x;
  }
}
#define RECORD(a, b, value)                                                                                    \
  do {                                                                                                         \
    __typeof__(value) result_ = (value);                                                                       \
    record(a, b, &result_, sizeof result_ == 16 ? 10 : sizeof result_);                                       \
  } while (0)

/* A conversion to an integer whose value is out of range (or a NaN) is undefined in C, and the native program's
 * value differs between -O0 and -O2: there only the invalid flag is compared (the inexact flag beside it is
 * unspecified, and clang's unsigned 64-bit sequence raises it for some operands). */
#define RECORD_INT(a, b, value)                                                                                \
  do {                                                                                                         \
    __typeof__(value) result_ = (value);                                                                       \
    if (fetestexcept(FE_INVALID)) {                                                                            \
      result_ = 0;                                                                                             \
      feclearexcept(FE_INEXACT);                                                                               \
    }                                                                                                          \
    record(a, b, &result_, sizeof result_);                                                                    \
  } while (0)

/* Which NaN comes back when two or more operands are NaNs is unspecified (x86's choice depends on the
 * instruction form), so then only that the result is a NaN is compared. */
#define RECORD_FMA(a, b, x, y, z, value)                                                                       \
  do {                                                                                                         \
    __typeof__(value) result_ = (value);                                                                       \
    if (__builtin_isnan(result_) && __builtin_isnan(x) + __builtin_isnan(y) + __builtin_isnan(z) >= 2)        \
      memset(&result_, 0xff, sizeof result_);                                                                  \
    record(a, b, &result_, sizeof result_ == 16 ? 10 : sizeof result_);                                       \
  } while (0)

/* Each group, over each mode. */
#define MODES for (mode_index = 0; mode_index < 4 && !fesetround(modes[mode_index]); ++mode_index)

#define BINARY(name, V, op)                                                                                    \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(V); ++i) for (unsigned j = 0; j < COUNT(V); ++j) {                   \
      start();                                                                                                 \
      RECORD(i, j, V[i] op V[j]);                                                                              \
    }                                                                                                          \
    end();                                                                                                     \
  }
BINARY(add_f, vf, +)
BINARY(sub_f, vf, -)
BINARY(mul_f, vf, *)
BINARY(div_f, vf, /)
BINARY(add_d, vd, +)
BINARY(sub_d, vd, -)
BINARY(mul_d, vd, *)
BINARY(div_d, vd, /)
BINARY(add_l, vl, +)
BINARY(sub_l, vl, -)
BINARY(mul_l, vl, *)
BINARY(div_l, vl, /)

/* The comparisons, as one word of bits: signalling <, <=, >, >=, quiet ==, !=, and the quiet comparisons of
 * <math.h> as the compiler's builtins, which glibc's macros are (musl's test for a NaN first, so they never
 * raise invalid, also not for a signalling NaN, whose behaviour C's Annex F leaves undefined). */
#define COMPARE(name, V)                                                                                       \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(V); ++i) for (unsigned j = 0; j < COUNT(V); ++j) {                   \
      int bits;                                                                                                \
      start();                                                                                                 \
      bits = V[i] < V[j];                                                                                      \
      RECORD(i, j, bits);                                                                                      \
      start();                                                                                                 \
      bits = (V[i] <= V[j]) | (V[i] > V[j]) << 1 | (V[i] >= V[j]) << 2;                                        \
      RECORD(i, j, bits);                                                                                      \
      start();                                                                                                 \
      bits = (V[i] == V[j]) | (V[i] != V[j]) << 1;                                                             \
      RECORD(i, j, bits);                                                                                      \
      start();                                                                                                 \
      bits = __builtin_isless(V[i], V[j]) | __builtin_isgreaterequal(V[i], V[j]) << 1 |                       \
             __builtin_islessgreater(V[i], V[j]) << 2 | __builtin_isunordered(V[i], V[j]) << 3;                \
      RECORD(i, j, bits);                                                                                      \
    }                                                                                                          \
    end();                                                                                                     \
  }
COMPARE(compare_f, vf)
COMPARE(compare_d, vd)
COMPARE(compare_l, vl)

#define UNARY(name, V, expression)                                                                             \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(V); ++i) {                                                            \
      start();                                                                                                 \
      __typeof__(V[0]) x = V[i];                                                                               \
      RECORD(i, 0, expression);                                                                                \
    }                                                                                                          \
    end();                                                                                                     \
  }
#define UNARY_INT(name, V, expression)                                                                         \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(V); ++i) {                                                            \
      start();                                                                                                 \
      __typeof__(V[0]) x = V[i];                                                                               \
      RECORD_INT(i, 0, expression);                                                                            \
    }                                                                                                          \
    end();                                                                                                     \
  }
UNARY(sqrt_f, vf, sqrtf(x))
UNARY(sqrt_d, vd, sqrt(x))
UNARY(sqrt_l, vl, sqrtl(x))
UNARY(to_d_f, vf, (double)x)
UNARY(to_l_f, vf, (long double)x)
UNARY(to_f_d, vd, (float)x)
UNARY(to_l_d, vd, (long double)x)
UNARY(to_f_l, vl, (float)x)
UNARY(to_d_l, vl, (double)x)
UNARY_INT(to_i32_f, vf, (int32_t)x)
UNARY_INT(to_i64_f, vf, (int64_t)x)
UNARY_INT(to_u32_f, vf, (uint32_t)x)
UNARY_INT(to_u64_f, vf, (uint64_t)x)
UNARY_INT(to_i16_f, vf, (int16_t)x)
UNARY_INT(to_u8_f, vf, (uint8_t)x)
UNARY_INT(to_i32_d, vd, (int32_t)x)
UNARY_INT(to_i64_d, vd, (int64_t)x)
UNARY_INT(to_u32_d, vd, (uint32_t)x)
UNARY_INT(to_u64_d, vd, (uint64_t)x)
UNARY_INT(to_i16_d, vd, (int16_t)x)
UNARY_INT(to_i32_l, vl, (int32_t)x)
UNARY_INT(to_i64_l, vl, (int64_t)x)
UNARY_INT(to_u32_l, vl, (uint32_t)x)
UNARY_INT(to_u64_l, vl, (uint64_t)x)
UNARY_INT(to_i16_l, vl, (int16_t)x)
UNARY(rint_f, vf, rintf(x))
UNARY(rint_d, vd, rint(x))
UNARY(rint_l, vl, rintl(x))
UNARY(nearbyint_f, vf, nearbyintf(x))
UNARY(nearbyint_d, vd, nearbyint(x))
UNARY(nearbyint_l, vl, nearbyintl(x))
UNARY(floor_f, vf, floorf(x))
UNARY(floor_d, vd, floor(x))
UNARY(floor_l, vl, floorl(x))
UNARY(ceil_d, vd, ceil(x))
UNARY(ceil_l, vl, ceill(x))
UNARY(trunc_d, vd, trunc(x))
UNARY(trunc_l, vl, truncl(x))
UNARY(round_f, vf, roundf(x))
UNARY(round_d, vd, round(x))
UNARY(round_l, vl, roundl(x))
UNARY(roundeven_d, vd, __builtin_roundeven(x))
UNARY(roundeven_l, vl, __builtin_roundevenl(x))
UNARY_INT(lrint_f, vf, lrintf(x))
UNARY_INT(lrint_d, vd, lrint(x))
UNARY_INT(lrint_l, vl, lrintl(x))
UNARY_INT(llrint_d, vd, llrint(x))
UNARY_INT(lround_d, vd, lround(x))
UNARY_INT(lround_l, vl, lroundl(x))
UNARY_INT(llround_f, vf, llroundf(x))

UNARY(to_h_f, vf, (_Float16)x)
UNARY(to_h_d, vd, (_Float16)x)
/* Not on the last two long doubles, an unnormal and a pseudo-denormal: libgcc's truncation, which the native
 * program calls, reads them at face value (`rt/softfloat/README.md`, "Noncanonical long double operands"). */
UNARY(to_h_l, vl, i >= COUNT(vl) - 2 ? (_Float16)0 : (_Float16)x)
/* To __bf16 outside FENV_ACCESS: under it clang 18 converts to the _Float16 encoding instead ((__bf16)1.0 is
 * 0x3c00), a native compiler defect. The conversion is a call of libgcc's, which reads the mode as it runs. */
#pragma STDC FENV_ACCESS OFF
static __attribute__((noinline)) __bf16 bf16_f(float x) { return (__bf16)x; }
static __attribute__((noinline)) __bf16 bf16_d(double x) { return (__bf16)x; }
#pragma STDC FENV_ACCESS ON
UNARY(to_bf_f, vf, bf16_f(x))
UNARY(to_bf_d, vd, bf16_d(x))
/* Not from long double: clang 18's back end cannot compile that conversion at all. */
UNARY(sqrt_h, vh, (_Float16)__builtin_sqrtf(x))
UNARY_INT(to_ti_h, vh, (__int128)x)
UNARY_INT(to_uti_h, vh, (unsigned __int128)x)
UNARY_INT(to_ti_d, vd, (__int128)x)
UNARY_INT(to_uti_d, vd, (unsigned __int128)x)
UNARY_INT(to_ti_l, vl, (__int128)x)
UNARY_INT(to_uti_l, vl, (unsigned __int128)x)
/* _Float16 arithmetic: of two NaN operands which one comes back is unspecified (native -O2 commutes the float
 * operation), so then only that the result is a NaN is compared. */
#define BINARY_HALF(name, op)                                                                                  \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(vh); ++i) for (unsigned j = 0; j < COUNT(vh); ++j) {                 \
      _Float16 x = vh[i], y = vh[j];                                                                           \
      start();                                                                                                 \
      RECORD_FMA(i, j, x, y, (_Float16)0, x op y);                                                             \
    }                                                                                                          \
    end();                                                                                                     \
  }
BINARY_HALF(add_h, +)
BINARY_HALF(mul_h, *)

/* Integers to floating point. */
static const int64_t integers[] = {
    0, 1, -1, 3, 16777217, -16777217, 0x7fffffff, -0x7fffffff - 1, 0x20000000000001, -0x20000000000003,
    0x7fffffffffffffff, -0x7fffffffffffffff - 1, 0x123456789abcdef, 0x7ffffff0000003ff, 1000000007, 129,
};
static volatile int64_t vi[COUNT(integers)];
#define FROM(name, T, F)                                                                                       \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(integers); ++i) {                                                     \
      start();                                                                                                 \
      RECORD(i, 0, (F)(T)vi[i]);                                                                               \
    }                                                                                                          \
    end();                                                                                                     \
  }
FROM(from_i32_f, int32_t, float)
FROM(from_i64_f, int64_t, float)
FROM(from_u32_f, uint32_t, float)
FROM(from_u64_f, uint64_t, float)
FROM(from_i64_d, int64_t, double)
FROM(from_u64_d, uint64_t, double)
FROM(from_i32_d, int32_t, double)
FROM(from_i64_l, int64_t, long double)
FROM(from_u64_l, uint64_t, long double)
FROM(from_i64_h, int64_t, _Float16)
/* Not an integer to __bf16: clang 18's x86-64 back end cannot compile it under FENV_ACCESS ("Do not know how
 * to soft promote this operator's result"), so there is no native reference. */
/* 128-bit integers: the 64-bit ones shifted left by 40 bits (some beyond 2^64, which overflows _Float16). */
#define FROM_WIDE(name, T, F)                                                                                  \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(integers); ++i) {                                                     \
      start();                                                                                                 \
      RECORD(i, 0, (F)((T)vi[i] * ((T)1 << 40)));                                                             \
    }                                                                                                          \
    end();                                                                                                     \
  }
FROM_WIDE(from_ti_d, __int128, double)
FROM_WIDE(from_uti_f, unsigned __int128, float)
FROM_WIDE(from_ti_l, __int128, long double)
FROM_WIDE(from_ti_h, __int128, _Float16)
FROM_WIDE(from_uti_h, unsigned __int128, _Float16)

/* Fused multiply-add on every triple of a smaller set (RECORD_FMA above). */
#define FMA(name, V, f)                                                                                        \
  static void name(void) {                                                                                     \
    static const unsigned picks[] = {0, 1, 2, 4, 6, 8, 9, 13, 14, 24, 26, 28, 29};                              \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(picks); ++i) for (unsigned j = 0; j < COUNT(picks); ++j)              \
        for (unsigned k = 0; k < COUNT(picks); ++k) {                                                          \
      __typeof__(V[0]) x = V[picks[i]], y = V[picks[j]], z = V[picks[k]];                                      \
      start();                                                                                                 \
      RECORD_FMA(i * 16 + j, k, x, y, z, f(x, y, z));                                                          \
    }                                                                                                          \
    end();                                                                                                     \
  }
FMA(fma_f, vf, fmaf)
FMA(fma_d, vd, fma)
FMA(fma_l, vl, fmal)

#define MINMAX(name, V, f)                                                                                     \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned i = 0; i < COUNT(V); ++i) for (unsigned j = 0; j < COUNT(V); ++j) {                   \
      start();                                                                                                 \
      RECORD(i, j, f(V[i], V[j]));                                                                             \
    }                                                                                                          \
    end();                                                                                                     \
  }
MINMAX(fmin_d, vd, fmin)
MINMAX(fmax_f, vf, fmaxf)
MINMAX(fmin_l, vl, fminl)

/* The C library's functions that musl writes under FENV_ACCESS, called by name (through a volatile pointer,
 * so that the compiler cannot treat them as builtins): ilogb is musl's, the rest the float runtime's
 * (`rt/musl/math.c`). */
static double (*volatile lib_nearbyint)(double) = nearbyint;
static float (*volatile lib_nearbyintf)(float) = nearbyintf;
static long double (*volatile lib_nearbyintl)(long double) = nearbyintl;
static long (*volatile lib_lrint)(double) = lrint;
static long (*volatile lib_lrintl)(long double) = lrintl;
static long long (*volatile lib_llrintl)(long double) = llrintl;
static int (*volatile lib_ilogb)(double) = ilogb;
static int (*volatile lib_ilogbf)(float) = ilogbf;
static int (*volatile lib_ilogbl)(long double) = ilogbl;
static double (*volatile lib_fma)(double, double, double) = fma;
static float (*volatile lib_fmaf)(float, float, float) = fmaf;
static long double (*volatile lib_fmal)(long double, long double, long double) = fmal;
UNARY(lib_nearbyint_d, vd, lib_nearbyint(x))
UNARY(lib_nearbyint_f, vf, lib_nearbyintf(x))
UNARY(lib_nearbyint_l, vl, lib_nearbyintl(x))
UNARY_INT(lib_lrint_d, vd, lib_lrint(x))
UNARY_INT(lib_lrint_l, vl, lib_lrintl(x))
UNARY_INT(lib_llrint_l, vl, lib_llrintl(x))
UNARY(lib_ilogb_d, vd, lib_ilogb(x))
UNARY(lib_ilogb_f, vf, lib_ilogbf(x))
/* Not on the last two long doubles, an unnormal and a pseudo-denormal, which no arithmetic produces: musl's ilogbl
 * reads their bits at face value, glibc's x87 code as x87 does. */
UNARY(lib_ilogb_l, vl, i >= COUNT(vl) - 2 ? 0 : lib_ilogbl(x))
FMA(lib_fma_d, vd, lib_fma)
FMA(lib_fma_f, vf, lib_fmaf)
FMA(lib_fma_l, vl, lib_fmal)

#ifdef RANDOM
/* Seeded random operands (-DRANDOM=n cases per group and mode, -DSEED=s): a mix of uniform bits, exponents at the
 * ends of the range and near 1, and small integers and halves. */
#ifndef SEED
#define SEED 1
#endif
static uint64_t state = SEED;
static uint64_t next(void) { /* splitmix64 */
  uint64_t z = (state += 0x9e3779b97f4a7c15u);
  z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9u;
  z = (z ^ (z >> 27)) * 0x94d049bb133111ebu;
  return z ^ (z >> 31);
}
static double random_d(void) {
  uint64_t r = next(), u;
  static const unsigned exponents[] = {0,    1,    2,    3,    52,   53,   54,   1020, 1021,
                                       1022, 1023, 1024, 1025, 2044, 2045, 2046, 2047};
  double d;
  switch (r & 3) {
  case 0: u = next(); break;
  case 1: u = (r >> 63) << 63 | (uint64_t)exponents[(r >> 2) % COUNT(exponents)] << 52 | (next() >> 12); break;
  case 2: d = (double)(int64_t)((r >> 2) % 4096 - 2048) / ((r >> 20 & 1) + 1); memcpy(&u, &d, 8); break;
  default: u = (r >> 63) << 63 | (uint64_t)(1023 + (r >> 2) % 9 - 4) << 52 | (next() >> 12); break;
  }
  memcpy(&d, &u, 8);
  return d;
}
static float random_f(void) {
  double d = random_d();
  uint64_t u;
  memcpy(&u, &d, 8);
  uint32_t v = (uint32_t)(u >> 32) ^ (uint32_t)u;
  if (next() & 1) v = (uint32_t)(u >> 63) << 31 | (uint32_t)((u >> 52 & 0x7ff) % 256) << 23 | (uint32_t)(u & 0x7fffff);
  float f;
  memcpy(&f, &v, 4);
  return f;
}
static long double random_l(void) {
  uint64_t r = next();
  uint16_t se = (uint16_t)((r & 1) ? r >> 48 : (r >> 63) << 15 | (0x3fff + (int)((r >> 8) % 140) - 70));
  uint64_t m = next() | (uint64_t)1 << 63;
  if ((se & 0x7fff) == 0) m >>= 1 + (r >> 20) % 63; /* a denormal or zero: canonical */
  long double x = 0;
  memcpy(&x, &m, 8);
  memcpy((char *)&x + 8, &se, 2);
  return x;
}
#define RANDOM_GROUP(name, T, draw, n, record, expression)                                                     \
  static void name(void) {                                                                                     \
    begin(#name);                                                                                              \
    MODES for (unsigned c = 0; c < RANDOM; ++c) {                                                              \
      volatile T x = draw(), y = draw(), z = draw();                                                           \
      (void)y;                                                                                                 \
      (void)z;                                                                                                 \
      start();                                                                                                 \
      record(c, n, expression);                                                                                \
    }                                                                                                          \
    end();                                                                                                     \
  }
#define RECORD_FMA3(a, b, value) RECORD_FMA(a, b, x, y, z, value)
RANDOM_GROUP(random_add_d, double, random_d, 0, RECORD, x + y)
RANDOM_GROUP(random_sub_d, double, random_d, 0, RECORD, x - y)
RANDOM_GROUP(random_mul_d, double, random_d, 0, RECORD, x * y)
RANDOM_GROUP(random_div_d, double, random_d, 0, RECORD, x / y)
RANDOM_GROUP(random_sqrt_d, double, random_d, 0, RECORD, sqrt(x))
RANDOM_GROUP(random_fma_d, double, random_d, 0, RECORD_FMA3, fma(x, y, z))
RANDOM_GROUP(random_add_f, float, random_f, 0, RECORD, x + y)
RANDOM_GROUP(random_mul_f, float, random_f, 0, RECORD, x * y)
RANDOM_GROUP(random_div_f, float, random_f, 0, RECORD, x / y)
RANDOM_GROUP(random_fma_f, float, random_f, 0, RECORD_FMA3, fmaf(x, y, z))
RANDOM_GROUP(random_add_l, long double, random_l, 0, RECORD, x + y)
RANDOM_GROUP(random_mul_l, long double, random_l, 0, RECORD, x * y)
RANDOM_GROUP(random_div_l, long double, random_l, 0, RECORD, x / y)
RANDOM_GROUP(random_sqrt_l, long double, random_l, 0, RECORD, sqrtl(x))
RANDOM_GROUP(random_fma_l, long double, random_l, 0, RECORD_FMA3, fmal(x, y, z))
RANDOM_GROUP(random_to_f_d, double, random_d, 0, RECORD, (float)x)
RANDOM_GROUP(random_to_d_l, long double, random_l, 0, RECORD, (double)x)
RANDOM_GROUP(random_to_i64_d, double, random_d, 0, RECORD_INT, (int64_t)x)
RANDOM_GROUP(random_to_u64_l, long double, random_l, 0, RECORD_INT, (uint64_t)x)
RANDOM_GROUP(random_rint_d, double, random_d, 0, RECORD, rint(x))
static void (*const groups[])(void) = {
    random_add_d,  random_sub_d,  random_mul_d,  random_div_d,  random_sqrt_d,   random_fma_d,  random_add_f,
    random_mul_f,  random_div_f,  random_fma_f,  random_add_l,  random_mul_l,    random_div_l,  random_sqrt_l,
    random_fma_l,  random_to_f_d, random_to_d_l, random_to_i64_d, random_to_u64_l, random_rint_d,
};
#else
static void (*const groups[])(void) = {
    add_f,        sub_f,        mul_f,         div_f,           add_d,           sub_d,          mul_d,
    div_d,        add_l,        sub_l,         mul_l,           div_l,           compare_f,      compare_d,
    compare_l,    sqrt_f,       sqrt_d,        sqrt_l,          to_d_f,          to_l_f,         to_f_d,
    to_l_d,       to_f_l,       to_d_l,        to_i32_f,        to_i64_f,        to_u32_f,       to_u64_f,
    to_i16_f,     to_u8_f,      to_i32_d,      to_i64_d,        to_u32_d,        to_u64_d,       to_i16_d,
    to_i32_l,     to_i64_l,     to_u32_l,      to_u64_l,        to_i16_l,        rint_f,         rint_d,
    rint_l,       nearbyint_f,  nearbyint_d,   nearbyint_l,     floor_f,         floor_d,        floor_l,
    ceil_d,       ceil_l,       trunc_d,       trunc_l,         round_f,         round_d,        round_l,
    roundeven_d,  roundeven_l,  lrint_f,       lrint_d,         lrint_l,         llrint_d,       lround_d,
    lround_l,     llround_f,    from_i32_f,    from_i64_f,      from_u32_f,      from_u64_f,     from_i64_d,
    from_u64_d,   from_i32_d,   from_i64_l,    from_u64_l,      fma_f,           fma_d,          fma_l,
    fmin_d,       fmax_f,       fmin_l,        lib_nearbyint_d, lib_nearbyint_f, lib_nearbyint_l, lib_lrint_d,
    lib_lrint_l,  lib_llrint_l, lib_ilogb_d,   lib_ilogb_f,     lib_ilogb_l,     lib_fma_d,      lib_fma_f,
    lib_fma_l,    to_h_f,       to_h_d,        to_h_l,          to_bf_f,         to_bf_d,
    sqrt_h,       to_ti_h,      to_uti_h,      to_ti_d,         to_uti_d,        to_ti_l,        to_uti_l,
    add_h,        mul_h,        from_i64_h,    from_ti_d,       from_uti_f,     from_ti_l,
    from_ti_h,    from_uti_h,
};
#endif

int main(void) {
  load();
  for (unsigned i = 0; i < COUNT(integers); ++i) vi[i] = integers[i];
  for (unsigned i = 0; i < COUNT(groups); ++i) {
#ifdef GROUP
    if (i != GROUP) continue;
#endif
    groups[i]();
  }
  fesetround(FE_TONEAREST);
  return 0;
}
