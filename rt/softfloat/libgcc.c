/* The compiler runtime-call interface for floating point, over Berkeley SoftFloat 3e.
 *
 * bennett-front rewrites each floating-point operation into a call of one of these functions, as LLVM's
 * soft-float legalisation does for a machine without floating-point hardware: the libgcc / compiler-rt
 * names and signatures (`double __adddf3(double, double)`), and for what has no libgcc name the C
 * library names LLVM's own RuntimeLibcalls.def uses (`sqrt`, `fma`, `floor`, `fmin`, ...). Any bitcode
 * that defines these names can replace this file's (`bennett-front --float-runtime`).
 *
 * Every operation rounds to nearest, ties to even, and produces the NaNs of x86 SSE (the 8086-SSE
 * specialisation): what the natively compiled program computes.
 *
 * Global state. SoftFloat keeps `softfloat_roundingMode`, `softfloat_detectTininess`,
 * `extF80_roundingPrecision` and `softfloat_exceptionFlags` in globals (softfloat_state.c). The first
 * three are never written: once bennett-front has made the runtime internal, LLVM's optimiser sees that
 * and folds their loads to the initial values. The exception flags are only ever or-ed into: C reads
 * them only through <fenv.h>, which is the C library's and not available, and an ordinary write could
 * never run under a qubit branch. So `softfloat_raiseFlags` here does nothing (the vendored one is not
 * compiled), and the optimiser deletes the flags and every write to them, as it deletes any internal
 * global that nothing reads.
 */

#include <stdbool.h>
#include <stdint.h>

#include "platform.h"
#include "softfloat.h"

/* What a comparison returns: libgcc's CMPtype, a machine word on x86-64, and compiler-rt's CMP_RESULT. */
typedef long cmp_t;

static float32_t f32_of(float x) {
  float32_t r;
  __builtin_memcpy(&r, &x, sizeof r);
  return r;
}
static float of_f32(float32_t x) {
  float r;
  __builtin_memcpy(&r, &x, sizeof r);
  return r;
}
static float64_t f64_of(double x) {
  float64_t r;
  __builtin_memcpy(&r, &x, sizeof r);
  return r;
}
static double of_f64(float64_t x) {
  double r;
  __builtin_memcpy(&r, &x, sizeof r);
  return r;
}

/* A long double is the x87 80-bit format in the low 10 of its 16 bytes: the significand with its explicit
 * integer bit, then the sign and exponent. Only those 10 bytes are copied: the rest is padding. */
static extFloat80_t x80_of(long double x) {
  extFloat80_t r;
  __builtin_memcpy(&r.signif, &x, 8);
  __builtin_memcpy(&r.signExp, (const char *)&x + 8, 2);
  return r;
}
static long double of_x80(extFloat80_t x) {
  long double r;
  __builtin_memcpy(&r, &x.signif, 8);
  __builtin_memcpy((char *)&r + 8, &x.signExp, 2);
  return r;
}

static bool nan32(float32_t x) { return (x.v & 0x7FFFFFFF) > 0x7F800000; }
static bool nan64(float64_t x) { return (x.v & UINT64_C(0x7FFFFFFFFFFFFFFF)) > UINT64_C(0x7FF0000000000000); }
/* SoftFloat's own test (isNaNExtF80UI): the largest exponent and a nonzero fraction, the integer bit aside. */
static bool nan80(extFloat80_t x) {
  return (x.signExp & 0x7FFF) == 0x7FFF && (x.signif & UINT64_C(0x7FFFFFFFFFFFFFFF)) != 0;
}

void softfloat_raiseFlags(uint_fast8_t flags) { (void)flags; }

/* Arithmetic. */
float __addsf3(float a, float b) { return of_f32(f32_add(f32_of(a), f32_of(b))); }
float __subsf3(float a, float b) { return of_f32(f32_sub(f32_of(a), f32_of(b))); }
float __mulsf3(float a, float b) { return of_f32(f32_mul(f32_of(a), f32_of(b))); }
float __divsf3(float a, float b) { return of_f32(f32_div(f32_of(a), f32_of(b))); }
double __adddf3(double a, double b) { return of_f64(f64_add(f64_of(a), f64_of(b))); }
double __subdf3(double a, double b) { return of_f64(f64_sub(f64_of(a), f64_of(b))); }
double __muldf3(double a, double b) { return of_f64(f64_mul(f64_of(a), f64_of(b))); }
double __divdf3(double a, double b) { return of_f64(f64_div(f64_of(a), f64_of(b))); }
/* long double rounds to the full 64-bit significand: extF80_roundingPrecision is never written and stays
 * 80 (x87 on Linux computes so by default). */
long double __addxf3(long double a, long double b) { return of_x80(extF80_add(x80_of(a), x80_of(b))); }
long double __subxf3(long double a, long double b) { return of_x80(extF80_sub(x80_of(a), x80_of(b))); }
long double __mulxf3(long double a, long double b) { return of_x80(extF80_mul(x80_of(a), x80_of(b))); }
long double __divxf3(long double a, long double b) { return of_x80(extF80_div(x80_of(a), x80_of(b))); }

/* Comparisons, as libgcc defines them: eq and ne return 0 exactly when a == b; lt and le return a
 * negative number, 0 or a positive number as a < b, a == b or a > b, and 1 when either is a NaN; gt and
 * ge the same, but -1 for a NaN; unord a nonzero number when either is a NaN. The quiet comparisons:
 * these never trap. */
static cmp_t order32(float32_t a, float32_t b, cmp_t unordered) {
  if (nan32(a) || nan32(b)) return unordered;
  return f32_lt_quiet(a, b) ? -1 : f32_eq(a, b) ? 0 : 1;
}
static cmp_t order64(float64_t a, float64_t b, cmp_t unordered) {
  if (nan64(a) || nan64(b)) return unordered;
  return f64_lt_quiet(a, b) ? -1 : f64_eq(a, b) ? 0 : 1;
}
static cmp_t order80(extFloat80_t a, extFloat80_t b, cmp_t unordered) {
  if (nan80(a) || nan80(b)) return unordered;
  return extF80_lt_quiet(a, b) ? -1 : extF80_eq(a, b) ? 0 : 1;
}
cmp_t __eqsf2(float a, float b) { return !f32_eq(f32_of(a), f32_of(b)); }
cmp_t __nesf2(float a, float b) { return !f32_eq(f32_of(a), f32_of(b)); }
cmp_t __ltsf2(float a, float b) { return order32(f32_of(a), f32_of(b), 1); }
cmp_t __lesf2(float a, float b) { return order32(f32_of(a), f32_of(b), 1); }
cmp_t __gtsf2(float a, float b) { return order32(f32_of(a), f32_of(b), -1); }
cmp_t __gesf2(float a, float b) { return order32(f32_of(a), f32_of(b), -1); }
cmp_t __unordsf2(float a, float b) { return nan32(f32_of(a)) || nan32(f32_of(b)); }
cmp_t __eqdf2(double a, double b) { return !f64_eq(f64_of(a), f64_of(b)); }
cmp_t __nedf2(double a, double b) { return !f64_eq(f64_of(a), f64_of(b)); }
cmp_t __ltdf2(double a, double b) { return order64(f64_of(a), f64_of(b), 1); }
cmp_t __ledf2(double a, double b) { return order64(f64_of(a), f64_of(b), 1); }
cmp_t __gtdf2(double a, double b) { return order64(f64_of(a), f64_of(b), -1); }
cmp_t __gedf2(double a, double b) { return order64(f64_of(a), f64_of(b), -1); }
cmp_t __unorddf2(double a, double b) { return nan64(f64_of(a)) || nan64(f64_of(b)); }
cmp_t __eqxf2(long double a, long double b) { return !extF80_eq(x80_of(a), x80_of(b)); }
cmp_t __nexf2(long double a, long double b) { return !extF80_eq(x80_of(a), x80_of(b)); }
cmp_t __ltxf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), 1); }
cmp_t __lexf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), 1); }
cmp_t __gtxf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), -1); }
cmp_t __gexf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), -1); }
cmp_t __unordxf2(long double a, long double b) { return nan80(x80_of(a)) || nan80(x80_of(b)); }

/* Conversions. To an integer: rounded toward zero, as C converts. Out of range is undefined in C; there the
 * signed conversions give what x86 SSE gives (the 8086-SSE specialisation's value), and the unsigned ones
 * what clang's x86-64 code gives for the same source: to 32 bits, the low half of the signed 64-bit
 * conversion; to 64 bits, the signed conversion below 2^63 (so a negative value wraps), 2^63 from 2^64 up
 * and for a NaN. For long double, x87 (`fistp`) gives the same: the indefinite integer, the least of the
 * width, out of range, and to 64 bits unsigned, the signed conversion below 2^63 and that of x - 2^63
 * with its top bit flipped from there on, which is 0 from 2^64 up and for a NaN. */
double __extendsfdf2(float a) { return of_f64(f32_to_f64(f32_of(a))); }
float __truncdfsf2(double a) { return of_f32(f64_to_f32(f64_of(a))); }
int __fixsfsi(float a) { return f32_to_i32_r_minMag(f32_of(a), false); }
long long __fixsfdi(float a) { return f32_to_i64_r_minMag(f32_of(a), false); }
unsigned __fixunssfsi(float a) { return (unsigned)f32_to_i64_r_minMag(f32_of(a), false); }
unsigned long long __fixunssfdi(float a) {
  float32_t x = f32_of(a), two63 = {0x5F000000}, two64 = {0x5F800000};
  return f32_lt_quiet(x, two63)   ? (unsigned long long)f32_to_i64_r_minMag(x, false)
         : f32_lt_quiet(x, two64) ? f32_to_ui64_r_minMag(x, false)
                                  : UINT64_C(0x8000000000000000);
}
int __fixdfsi(double a) { return f64_to_i32_r_minMag(f64_of(a), false); }
long long __fixdfdi(double a) { return f64_to_i64_r_minMag(f64_of(a), false); }
unsigned __fixunsdfsi(double a) { return (unsigned)f64_to_i64_r_minMag(f64_of(a), false); }
unsigned long long __fixunsdfdi(double a) {
  float64_t x = f64_of(a), two63 = {UINT64_C(0x43E0000000000000)}, two64 = {UINT64_C(0x43F0000000000000)};
  return f64_lt_quiet(x, two63)   ? (unsigned long long)f64_to_i64_r_minMag(x, false)
         : f64_lt_quiet(x, two64) ? f64_to_ui64_r_minMag(x, false)
                                  : UINT64_C(0x8000000000000000);
}
long double __extendsfxf2(float a) { return of_x80(f32_to_extF80(f32_of(a))); }
long double __extenddfxf2(double a) { return of_x80(f64_to_extF80(f64_of(a))); }
float __truncxfsf2(long double a) { return of_f32(extF80_to_f32(x80_of(a))); }
double __truncxfdf2(long double a) { return of_f64(extF80_to_f64(x80_of(a))); }
int __fixxfsi(long double a) { return extF80_to_i32_r_minMag(x80_of(a), false); }
long long __fixxfdi(long double a) { return extF80_to_i64_r_minMag(x80_of(a), false); }
unsigned __fixunsxfsi(long double a) { return (unsigned)extF80_to_i64_r_minMag(x80_of(a), false); }
unsigned long long __fixunsxfdi(long double a) {
  extFloat80_t x = x80_of(a), two63 = {UINT64_C(0x8000000000000000), 0x403E},
               two64 = {UINT64_C(0x8000000000000000), 0x403F};
  return extF80_lt_quiet(x, two63)   ? (unsigned long long)extF80_to_i64_r_minMag(x, false)
         : extF80_lt_quiet(x, two64) ? extF80_to_ui64_r_minMag(x, false)
                                     : 0;
}
float __floatsisf(int a) { return of_f32(i32_to_f32(a)); }
float __floatdisf(long long a) { return of_f32(i64_to_f32(a)); }
float __floatunsisf(unsigned a) { return of_f32(ui32_to_f32(a)); }
float __floatundisf(unsigned long long a) { return of_f32(ui64_to_f32(a)); }
double __floatsidf(int a) { return of_f64(i32_to_f64(a)); }
double __floatdidf(long long a) { return of_f64(i64_to_f64(a)); }
double __floatunsidf(unsigned a) { return of_f64(ui32_to_f64(a)); }
double __floatundidf(unsigned long long a) { return of_f64(ui64_to_f64(a)); }
long double __floatsixf(int a) { return of_x80(i32_to_extF80(a)); }
long double __floatdixf(long long a) { return of_x80(i64_to_extF80(a)); }
long double __floatunsixf(unsigned a) { return of_x80(ui32_to_extF80(a)); }
long double __floatundixf(unsigned long long a) { return of_x80(ui64_to_extF80(a)); }

/* x to the power m by repeated squaring, as libgcc2.c computes it (`__builtin_powi`). */
float __powisf2(float x, int m) {
  unsigned n = m < 0 ? -(unsigned)m : (unsigned)m;
  float32_t a = f32_of(x), one = {0x3F800000}, y = n % 2 ? a : one;
  while (n >>= 1) {
    a = f32_mul(a, a);
    if (n % 2) y = f32_mul(y, a);
  }
  return of_f32(m < 0 ? f32_div(one, y) : y);
}
double __powidf2(double x, int m) {
  unsigned n = m < 0 ? -(unsigned)m : (unsigned)m;
  float64_t a = f64_of(x), one = {UINT64_C(0x3FF0000000000000)}, y = n % 2 ? a : one;
  while (n >>= 1) {
    a = f64_mul(a, a);
    if (n % 2) y = f64_mul(y, a);
  }
  return of_f64(m < 0 ? f64_div(one, y) : y);
}

long double __powixf2(long double x, int m) {
  unsigned n = m < 0 ? -(unsigned)m : (unsigned)m;
  extFloat80_t a = x80_of(x), one = {UINT64_C(0x8000000000000000), 0x3FFF}, y = n % 2 ? a : one;
  while (n >>= 1) {
    a = extF80_mul(a, a);
    if (n % 2) y = extF80_mul(y, a);
  }
  return of_x80(m < 0 ? extF80_div(one, y) : y);
}

/* The C library names LLVM uses for the operations libgcc has no name for. Correctly rounded, as IEEE
 * 754 defines them; errno is not set (it is the C library's, and these are not its functions). */
float sqrtf(float a) { return of_f32(f32_sqrt(f32_of(a))); }
double sqrt(double a) { return of_f64(f64_sqrt(f64_of(a))); }
float fmaf(float a, float b, float c) { return of_f32(f32_mulAdd(f32_of(a), f32_of(b), f32_of(c))); }
double fma(double a, double b, double c) { return of_f64(f64_mulAdd(f64_of(a), f64_of(b), f64_of(c))); }
long double sqrtl(long double a) { return of_x80(extF80_sqrt(x80_of(a))); }
/* No fmal: SoftFloat has no extF80 fused multiply-add. */

/* Rounding to an integral value: rint and nearbyint in the current mode, which is always to nearest. */
#define ROUND(name, mode)                                                                                  \
  float name##f(float a) { return of_f32(f32_roundToInt(f32_of(a), mode, false)); }                       \
  double name(double a) { return of_f64(f64_roundToInt(f64_of(a), mode, false)); }                       \
  long double name##l(long double a) { return of_x80(extF80_roundToInt(x80_of(a), mode, false)); }
ROUND(floor, softfloat_round_min)
ROUND(ceil, softfloat_round_max)
ROUND(trunc, softfloat_round_minMag)
ROUND(round, softfloat_round_near_maxMag)
ROUND(roundeven, softfloat_round_near_even)
ROUND(rint, softfloat_round_near_even)
ROUND(nearbyint, softfloat_round_near_even)

/* fmin and fmax (`llvm.minnum`, `llvm.maxnum`): what clang's x86-64 code computes for them, which is not a
 * call but SSE's minsd/maxsd with a NaN test (X86ISelLowering's combineFMinNumFMaxNum): fmin(x, y) is
 * `isnan(x) ? y : y < x ? y : x`, and fmax the same with `>`. So a NaN operand gives the other operand, and of
 * two equal operands (+0 and -0 among them) the first. C allows either zero; this is the one the native
 * program gives. (The C library's own fmin, called only under -fno-builtin, keeps the second.) */
float fminf(float x, float y) {
  float32_t a = f32_of(x), b = f32_of(y);
  return nan32(a) ? y : f32_lt_quiet(b, a) ? y : x;
}
float fmaxf(float x, float y) {
  float32_t a = f32_of(x), b = f32_of(y);
  return nan32(a) ? y : f32_lt_quiet(a, b) ? y : x;
}
double fmin(double x, double y) {
  float64_t a = f64_of(x), b = f64_of(y);
  return nan64(a) ? y : f64_lt_quiet(b, a) ? y : x;
}
double fmax(double x, double y) {
  float64_t a = f64_of(x), b = f64_of(y);
  return nan64(a) ? y : f64_lt_quiet(a, b) ? y : x;
}

/* fminl and fmaxl: on x86-64 the native program calls the C library's (LLVM has no inline expansion for
 * long double), and these are glibc's (sysdeps/x86_64/fpu/s_fminl.S and s_fmaxl.S, x87 code): of equal
 * operands, fminl gives the first and fmaxl the second; a NaN operand gives the other operand, unless
 * both are NaNs or either is signalling, which gives their sum (a quiet NaN, by x87's rules). */
static long double unordered80(long double x, long double y) {
  extFloat80_t a = x80_of(x), b = x80_of(y);
  if (extF80_isSignalingNaN(a) || extF80_isSignalingNaN(b) || (nan80(a) && nan80(b)))
    return of_x80(extF80_add(a, b));
  return nan80(a) ? y : x;
}
long double fminl(long double x, long double y) {
  extFloat80_t a = x80_of(x), b = x80_of(y);
  if (nan80(a) || nan80(b)) return unordered80(x, y);
  return extF80_lt_quiet(b, a) ? y : x;
}
long double fmaxl(long double x, long double y) {
  extFloat80_t a = x80_of(x), b = x80_of(y);
  if (nan80(a) || nan80(b)) return unordered80(x, y);
  return extF80_lt_quiet(b, a) ? x : y;
}
