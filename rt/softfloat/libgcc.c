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

static bool nan32(float32_t x) { return (x.v & 0x7FFFFFFF) > 0x7F800000; }
static bool nan64(float64_t x) { return (x.v & UINT64_C(0x7FFFFFFFFFFFFFFF)) > UINT64_C(0x7FF0000000000000); }

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

/* Conversions. To an integer: rounded toward zero, as C converts. Out of range is undefined in C; there the
 * signed conversions give what x86 SSE gives (the 8086-SSE specialisation's value), and the unsigned ones
 * what clang's x86-64 code gives for the same source: to 32 bits, the low half of the signed 64-bit
 * conversion; to 64 bits, the signed conversion below 2^63 (so a negative value wraps), 2^63 from 2^64 up
 * and for a NaN. */
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
float __floatsisf(int a) { return of_f32(i32_to_f32(a)); }
float __floatdisf(long long a) { return of_f32(i64_to_f32(a)); }
float __floatunsisf(unsigned a) { return of_f32(ui32_to_f32(a)); }
float __floatundisf(unsigned long long a) { return of_f32(ui64_to_f32(a)); }
double __floatsidf(int a) { return of_f64(i32_to_f64(a)); }
double __floatdidf(long long a) { return of_f64(i64_to_f64(a)); }
double __floatunsidf(unsigned a) { return of_f64(ui32_to_f64(a)); }
double __floatundidf(unsigned long long a) { return of_f64(ui64_to_f64(a)); }

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

/* The C library names LLVM uses for the operations libgcc has no name for. Correctly rounded, as IEEE
 * 754 defines them; errno is not set (it is the C library's, and these are not its functions). */
float sqrtf(float a) { return of_f32(f32_sqrt(f32_of(a))); }
double sqrt(double a) { return of_f64(f64_sqrt(f64_of(a))); }
float fmaf(float a, float b, float c) { return of_f32(f32_mulAdd(f32_of(a), f32_of(b), f32_of(c))); }
double fma(double a, double b, double c) { return of_f64(f64_mulAdd(f64_of(a), f64_of(b), f64_of(c))); }

/* Rounding to an integral value: rint and nearbyint in the current mode, which is always to nearest. */
#define ROUND(name, mode)                                                                                  \
  float name##f(float a) { return of_f32(f32_roundToInt(f32_of(a), mode, false)); }                       \
  double name(double a) { return of_f64(f64_roundToInt(f64_of(a), mode, false)); }
ROUND(floor, softfloat_round_min)
ROUND(ceil, softfloat_round_max)
ROUND(trunc, softfloat_round_minMag)
ROUND(round, softfloat_round_near_maxMag)
ROUND(roundeven, softfloat_round_near_even)
ROUND(rint, softfloat_round_near_even)
ROUND(nearbyint, softfloat_round_near_even)

/* fmin and fmax: a NaN operand gives the other operand (a NaN if both are), and -0 is taken as less
 * than +0 (C allows either zero; this is IEEE 754-2019 minimumNumber and maximumNumber). */
static bool less32(float32_t a, float32_t b) {
  return f32_lt_quiet(a, b) || (f32_eq(a, b) && (a.v >> 31) > (b.v >> 31));
}
static bool less64(float64_t a, float64_t b) {
  return f64_lt_quiet(a, b) || (f64_eq(a, b) && (a.v >> 63) > (b.v >> 63));
}
float fminf(float x, float y) {
  float32_t a = f32_of(x), b = f32_of(y);
  return nan32(a) ? y : nan32(b) ? x : less32(b, a) ? y : x;
}
float fmaxf(float x, float y) {
  float32_t a = f32_of(x), b = f32_of(y);
  return nan32(a) ? y : nan32(b) ? x : less32(a, b) ? y : x;
}
double fmin(double x, double y) {
  float64_t a = f64_of(x), b = f64_of(y);
  return nan64(a) ? y : nan64(b) ? x : less64(b, a) ? y : x;
}
double fmax(double x, double y) {
  float64_t a = f64_of(x), b = f64_of(y);
  return nan64(a) ? y : nan64(b) ? x : less64(a, b) ? y : x;
}
