/* Rounding to a 64-bit integer at the edges of its range: lround, llround, lrint and llrint on float, double and
 * long double, called directly and through function pointers, under each of the four rounding modes, on the
 * values just inside, at and just outside -2^63 and 2^63 (both signs), and on infinities and NaNs. Each case
 * prints the function, the mode, the operand's bits, the result's bits and the flags each unit raised (SSE's
 * MXCSR and x87's status word). Compiled natively (glibc) and for the device (musl), run, and the outputs
 * compared line for line by `front/tests/tests/fenv.rs`.
 *
 * At exponent 62 a long double has a half: 2^63 - 0.5 rounds half away from zero (lround) or to even (lrint
 * to nearest) to 2^63, outside the range, which glibc's lroundl and llroundl signal by raising invalid with
 * feraiseexcept (in SSE), and x87's fistp by raising invalid in x87. Operands come through volatile memory. */
#include <fenv.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#pragma STDC FENV_ACCESS ON

#define COUNT(a) (sizeof(a) / sizeof((a)[0]))

static const int modes[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
static const char *const mode_names[4] = {"near", "down", "up", "zero"};

/* Magnitudes; each is tried with both signs. */
static volatile float vf[] = {
    0x1.fffffep22f,  /* 2^23 - 0.5, the last half */
    0x1.fffffep62f,  /* the largest float below 2^63 */
    0x1p63f,         /* 2^63 */
    0x1.000002p63f,  /* the next float */
    0x1p64f,         /* 2^64 */
    __builtin_inff(),
    __builtin_nanf(""),
};
static volatile double vd[] = {
    0x1.fffffffffffffp51,  /* 2^52 - 0.5, the last half */
    0x1.fffffffffffffp62,  /* the largest double below 2^63 */
    0x1p63,                /* 2^63 */
    0x1.0000000000001p63,  /* the next double */
    0x1p64,                /* 2^64 */
    __builtin_inf(),
    __builtin_nan(""),
};
static volatile long double vl[] = {
    0x1.fffffffcp30L,            /* 2^31 - 0.5 */
    0x1.fffffffep31L,            /* 2^32 - 0.5, the last half of the high 32 bits */
    0x1.0000000000000002p62L,    /* 2^62 + 0.5 */
    0x1.fffffffffffffffap62L,    /* 2^63 - 1.5 */
    0x1.fffffffffffffffcp62L,    /* 2^63 - 1 */
    0x1.fffffffffffffffep62L,    /* 2^63 - 0.5, the last half */
    0x1p63L,                     /* 2^63 */
    0x1.0000000000000002p63L,    /* 2^63 + 1 */
    0x1.0000000000000004p63L,    /* 2^63 + 2 */
    0x1.fffffffffffffffep63L,    /* 2^64 - 1 */
    0x1p64L,                     /* 2^64 */
    __builtin_infl(),
    __builtin_nanl(""),
};

static void bits(const void *p, size_t n) {
  const unsigned char *b = p;
  for (size_t i = n; i-- > 0;) printf("%02x", b[i]);
}

static void record(const char *name, int mode, const void *operand, size_t size, long long result) {
  fenv_t e;
  fegetenv(&e);
  printf("%s %s ", name, mode_names[mode]);
  bits(operand, size);
  printf(" %016llx sse %02x x87 %02x\n", (unsigned long long)result, e.__mxcsr & FE_ALL_EXCEPT,
         e.__status_word & FE_ALL_EXCEPT);
}

static long (*volatile p_lroundf)(float) = lroundf;
static long long (*volatile p_llroundf)(float) = llroundf;
static long (*volatile p_lrintf)(float) = lrintf;
static long long (*volatile p_llrintf)(float) = llrintf;
static long (*volatile p_lround)(double) = lround;
static long long (*volatile p_llround)(double) = llround;
static long (*volatile p_lrint)(double) = lrint;
static long long (*volatile p_llrint)(double) = llrint;
static long (*volatile p_lroundl)(long double) = lroundl;
static long long (*volatile p_llroundl)(long double) = llroundl;
static long (*volatile p_lrintl)(long double) = lrintl;
static long long (*volatile p_llrintl)(long double) = llrintl;

/* One function over the values of V (an array of one type) and their negatives, in every mode. */
#define CASES(name, V, SIZE, expression)                                                                        \
  static void name(void) {                                                                                     \
    for (int m = 0; m < 4; ++m) {                                                                              \
      for (unsigned i = 0; i < 2 * COUNT(V); ++i) {                                                            \
        __typeof__(V[0] + 0) x = i % 2 ? -V[i / 2] : V[i / 2];                                                 \
        fesetround(modes[m]);                                                                                  \
        feclearexcept(FE_ALL_EXCEPT);                                                                          \
        long long result = (expression);                                                                       \
        fesetround(FE_TONEAREST);                                                                              \
        record(#name, m, &x, SIZE, result);                                                                    \
        feclearexcept(FE_ALL_EXCEPT);                                                                          \
      }                                                                                                        \
    }                                                                                                          \
  }
CASES(lroundf_, vf, 4, lroundf(x))
CASES(llroundf_, vf, 4, llroundf(x))
CASES(lrintf_, vf, 4, lrintf(x))
CASES(llrintf_, vf, 4, llrintf(x))
CASES(lround_, vd, 8, lround(x))
CASES(llround_, vd, 8, llround(x))
CASES(lrint_, vd, 8, lrint(x))
CASES(llrint_, vd, 8, llrint(x))
CASES(lroundl_, vl, 10, lroundl(x))
CASES(llroundl_, vl, 10, llroundl(x))
CASES(lrintl_, vl, 10, lrintl(x))
CASES(llrintl_, vl, 10, llrintl(x))
CASES(lib_lroundf, vf, 4, p_lroundf(x))
CASES(lib_llroundf, vf, 4, p_llroundf(x))
CASES(lib_lrintf, vf, 4, p_lrintf(x))
CASES(lib_llrintf, vf, 4, p_llrintf(x))
CASES(lib_lround, vd, 8, p_lround(x))
CASES(lib_llround, vd, 8, p_llround(x))
CASES(lib_lrint, vd, 8, p_lrint(x))
CASES(lib_llrint, vd, 8, p_llrint(x))
CASES(lib_lroundl, vl, 10, p_lroundl(x))
CASES(lib_llroundl, vl, 10, p_llroundl(x))
CASES(lib_lrintl, vl, 10, p_lrintl(x))
CASES(lib_llrintl, vl, 10, p_llrintl(x))

int main(void) {
  lroundf_();
  llroundf_();
  lrintf_();
  llrintf_();
  lround_();
  llround_();
  lrint_();
  llrint_();
  lroundl_();
  llroundl_();
  lrintl_();
  llrintl_();
  lib_lroundf();
  lib_llroundf();
  lib_lrintf();
  lib_llrintf();
  lib_lround();
  lib_llround();
  lib_lrint();
  lib_llrint();
  lib_lroundl();
  lib_llroundl();
  lib_lrintl();
  lib_llrintl();
  return 0;
}
