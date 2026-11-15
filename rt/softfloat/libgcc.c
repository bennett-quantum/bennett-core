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
#include "internals.h" /* SoftFloat's own rounding (softfloat_roundPackTo...) for the routines it has no entry for */

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

/* Conversions between floating point and 128-bit integers (`__fixdfti`, `__floattidf`, ...).
 *
 * From an integer: correctly rounded, by SoftFloat's own rounding: the magnitude shifted until its bit 127 is
 * set, its high 64 bits the significand and the low 64 jammed into its lowest bit (extF80 takes them as its
 * extra word), packed as SoftFloat's ui64_to_* pack a 64-bit integer whose top bit is set.
 *
 * To an integer: the native program calls libgcc's routines, which on x86-64 are libgcc2.c's (`__fixunsdfDI`,
 * `__fixunsxfDI`, compiled by GCC to SSE and x87 instructions). They are followed here step for step, so that
 * what an out-of-range value or a NaN gives (undefined in C) is what the native routine gives: the high word is
 * x / 2^64 converted to 64 bits unsigned, the low word what is left converted likewise; a negative x goes
 * through -x and the negated result for the signed conversions, and gives 0 for the unsigned long double one.
 * GCC converts to 64 bits unsigned as `x >= 2^63 ? (x - 2^63 converted signed) ^ 2^63 : x converted signed`, a
 * NaN taking the second branch, and the signed conversion of a value out of range is 2^63 (cvttsd2si, fistp).
 * float goes through double, as libgcc2.c's __fixunssfDI does. */
typedef __int128 ti_t;
typedef unsigned __int128 uti_t;

/* a, not 0, shifted left until its bit 127 is set: the high and the low 64 bits, and the shift. */
struct normalised {
  uint64_t sig, extra;
  int_fast16_t shift;
};
static struct normalised normalised(uti_t a) {
  uint64_t hi = (uint64_t)(a >> 64);
  int_fast16_t shift = hi ? softfloat_countLeadingZeros64(hi) : 64 + softfloat_countLeadingZeros64((uint64_t)a);
  a <<= shift;
  struct normalised n = {(uint64_t)(a >> 64), (uint64_t)a, shift};
  return n;
}
static float32_t f32_of_ti(bool sign, uti_t a) {
  if (!a) return (float32_t){0};
  struct normalised n = normalised(a);
  return softfloat_roundPackToF32(sign, 0xBD + 64 - n.shift, softfloat_shortShiftRightJam64(n.sig | (n.extra != 0), 33));
}
static float64_t f64_of_ti(bool sign, uti_t a) {
  if (!a) return (float64_t){0};
  struct normalised n = normalised(a);
  return softfloat_roundPackToF64(sign, 0x43D + 64 - n.shift, softfloat_shortShiftRightJam64(n.sig | (n.extra != 0), 1));
}
static extFloat80_t x80_of_ti(bool sign, uti_t a) {
  if (!a) return (extFloat80_t){0, 0};
  struct normalised n = normalised(a);
  return softfloat_roundPackToExtF80(sign, 0x407E - n.shift, n.sig, n.extra, extF80_roundingPrecision);
}
static uti_t magnitude(ti_t a) { return a < 0 ? -(uti_t)a : (uti_t)a; }
float __floattisf(ti_t a) { return of_f32(f32_of_ti(a < 0, magnitude(a))); }
float __floatuntisf(uti_t a) { return of_f32(f32_of_ti(false, a)); }
double __floattidf(ti_t a) { return of_f64(f64_of_ti(a < 0, magnitude(a))); }
double __floatuntidf(uti_t a) { return of_f64(f64_of_ti(false, a)); }
long double __floattixf(ti_t a) { return of_x80(x80_of_ti(a < 0, magnitude(a))); }
long double __floatuntixf(uti_t a) { return of_x80(x80_of_ti(false, a)); }

static uint64_t u64_of_f64_gcc(float64_t x) {
  const float64_t two63 = {UINT64_C(0x43E0000000000000)};
  if (f64_le_quiet(two63, x)) return (uint64_t)f64_to_i64_r_minMag(f64_sub(x, two63), false) ^ UINT64_C(0x8000000000000000);
  return (uint64_t)f64_to_i64_r_minMag(x, false);
}
static uti_t uti_of_f64(float64_t x) {
  const float64_t twoM64 = {UINT64_C(0x3BF0000000000000)}, two64 = {UINT64_C(0x43F0000000000000)};
  uint64_t hi = u64_of_f64_gcc(f64_mul(twoM64, x));
  uint64_t lo = u64_of_f64_gcc(f64_sub(x, f64_mul(ui64_to_f64(hi), two64)));
  return (uti_t)hi << 64 | lo;
}
static ti_t ti_of_f64(float64_t x) {
  const float64_t zero = {0};
  if (f64_lt_quiet(x, zero)) return (ti_t)-uti_of_f64((float64_t){x.v ^ UINT64_C(0x8000000000000000)});
  return (ti_t)uti_of_f64(x);
}
ti_t __fixsfti(float a) { return ti_of_f64(f32_to_f64(f32_of(a))); }
uti_t __fixunssfti(float a) { return uti_of_f64(f32_to_f64(f32_of(a))); }
ti_t __fixdfti(double a) { return ti_of_f64(f64_of(a)); }
uti_t __fixunsdfti(double a) { return uti_of_f64(f64_of(a)); }

static uint64_t u64_of_x80_gcc(extFloat80_t x) {
  const extFloat80_t two63 = {UINT64_C(0x8000000000000000), 0x403E};
  if (extF80_le_quiet(two63, x)) return (uint64_t)extF80_to_i64_r_minMag(extF80_sub(x, two63), false) ^ UINT64_C(0x8000000000000000);
  return (uint64_t)extF80_to_i64_r_minMag(x, false);
}
static uti_t uti_of_x80(extFloat80_t x) {
  const extFloat80_t zero = {0, 0}, twoM64 = {UINT64_C(0x8000000000000000), 0x3FBF};
  if (extF80_lt_quiet(x, zero)) return 0;
  uti_t v = (uti_t)u64_of_x80_gcc(extF80_mul(x, twoM64)) << 64;
  extFloat80_t rest = extF80_sub(x, x80_of_ti(false, v));
  if (extF80_lt_quiet(rest, zero)) return v - u64_of_x80_gcc((extFloat80_t){rest.signif, (uint16_t)(rest.signExp ^ 0x8000)});
  return v + u64_of_x80_gcc(rest);
}
ti_t __fixxfti(long double a) {
  extFloat80_t x = x80_of(a), zero = {0, 0};
  if (extF80_lt_quiet(x, zero)) return (ti_t)-uti_of_x80((extFloat80_t){x.signif, (uint16_t)(x.signExp ^ 0x8000)});
  return (ti_t)uti_of_x80(x);
}
uti_t __fixunsxfti(long double a) { return uti_of_x80(x80_of(a)); }

/* _Float16 and __bf16. The native program computes them as LLVM's x86-64 back end does without
 * half-precision hardware: every operation in float, rounded back after each, through these conversions.
 * For _Float16 they are SoftFloat's (one rounding each); __fixhfti and friends follow libgcc's soft-fp
 * routines, which saturate: an infinity or a NaN gives the largest or least integer by its sign, and the
 * unsigned conversion of a negative value 0. A _Float16 never needs more than 17 integer bits, so the 64-bit
 * conversions do the rest; a 128-bit integer of 2^64 or more is beyond its range and rounds to infinity. */
static float16_t f16_of(_Float16 x) {
  float16_t r;
  __builtin_memcpy(&r, &x, sizeof r);
  return r;
}
static _Float16 of_f16(float16_t x) {
  _Float16 r;
  __builtin_memcpy(&r, &x, sizeof r);
  return r;
}
float __extendhfsf2(_Float16 a) { return of_f32(f16_to_f32(f16_of(a))); }
long double __extendhfxf2(_Float16 a) { return of_x80(f16_to_extF80(f16_of(a))); }
_Float16 __truncsfhf2(float a) { return of_f16(f32_to_f16(f32_of(a))); }
_Float16 __truncdfhf2(double a) { return of_f16(f64_to_f16(f64_of(a))); }
_Float16 __truncxfhf2(long double a) { return of_f16(extF80_to_f16(x80_of(a))); }
ti_t __fixhfti(_Float16 a) {
  float16_t x = f16_of(a);
  if ((x.v & 0x7C00) == 0x7C00) return x.v >> 15 ? (ti_t)((uti_t)1 << 127) : (ti_t)(~(uti_t)0 >> 1);
  return f16_to_i64_r_minMag(x, false);
}
uti_t __fixunshfti(_Float16 a) {
  float16_t x = f16_of(a);
  if (x.v >> 15) return 0;
  if ((x.v & 0x7C00) == 0x7C00) return ~(uti_t)0;
  return f16_to_ui64_r_minMag(x, false);
}
_Float16 __floattihf(ti_t a) {
  float16_t inf = {(uint16_t)(a < 0 ? 0xFC00 : 0x7C00)};
  return of_f16(a == (int64_t)a ? i64_to_f16((int64_t)a) : inf);
}
_Float16 __floatuntihf(uti_t a) {
  float16_t inf = {0x7C00};
  return of_f16(a >> 64 ? inf : ui64_to_f16((uint64_t)a));
}

/* __bf16 is the top half of a float, which is all an extension does (the front writes it inline). SoftFloat
 * has no bfloat16, so the roundings to it are written here: from float, to nearest even on the bits (a carry
 * runs into the exponent, which is right up to infinity), and a NaN quiet with the top of its payload, as
 * libgcc's __truncsfbf2. From double and long double: rounded to float to odd (a float that is inexact is
 * made odd towards the exact value, overflow giving the largest finite float), then to __bf16, which is one
 * correct rounding because float keeps at least two bits more than __bf16 everywhere, subnormals included
 * (the exponent ranges are the same). */
static __bf16 bf16_of_bits(uint32_t u) {
  uint16_t r = (u & 0x7FFFFFFF) > 0x7F800000 ? (uint16_t)(u >> 16 | 0x0040) : (uint16_t)((u + 0x7FFF + (u >> 16 & 1)) >> 16);
  __bf16 b;
  __builtin_memcpy(&b, &r, sizeof b);
  return b;
}
static uint32_t odd_step(float32_t r, bool inexact, bool below) {
  return inexact && !(r.v & 1) ? (below ? r.v + 1 : r.v - 1) : r.v;
}
__bf16 __truncsfbf2(float a) { return bf16_of_bits(f32_of(a).v); }
__bf16 __truncdfbf2(double a) {
  float64_t x = f64_of(a), ax = {x.v & UINT64_C(0x7FFFFFFFFFFFFFFF)};
  float32_t r = f64_to_f32(x);
  float64_t back = f32_to_f64(r), ab = {back.v & UINT64_C(0x7FFFFFFFFFFFFFFF)};
  return bf16_of_bits(odd_step(r, !nan64(x) && !f64_eq(back, x), f64_lt_quiet(ab, ax)));
}
__bf16 __truncxfbf2(long double a) {
  extFloat80_t x = x80_of(a), ax = {x.signif, (uint16_t)(x.signExp & 0x7FFF)};
  float32_t r = extF80_to_f32(x);
  extFloat80_t back = f32_to_extF80(r), ab = {back.signif, (uint16_t)(back.signExp & 0x7FFF)};
  return bf16_of_bits(odd_step(r, !nan80(x) && !extF80_eq(back, x), extF80_lt_quiet(ab, ax)));
}

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

/* fmal: SoftFloat has no extF80 fused multiply-add, so it is written here over SoftFloat's primitives and
 * rounded once by its softfloat_roundPackToExtF80. The native program calls glibc's (ldbl-96 s_fmal.c), whose
 * special cases are computed with x87 operations, which SoftFloat's extF80 operations match: an infinite or
 * NaN addend with finite factors gives (z + x) + y; a zero addend with nonzero factors x * y; an infinite or
 * NaN operand or a zero factor x * y + z. Otherwise the exact product of the two 64-bit significands (128
 * bits) and the addend's are aligned in a window of 256 bits, the smaller shifted right with the bits it
 * loses jammed into the lowest one, added or subtracted, normalised, and the top 64 bits rounded with the
 * next 64 (and the rest jammed) as the extra word. A jammed bit stays far below the rounding position: a
 * subtraction cancels more than one leading bit only when the exponents differ by at most one, and then no
 * bit was shifted out. */
static bool special80(extFloat80_t x) { return (x.signExp & 0x7FFF) == 0x7FFF; }
static bool zero80(extFloat80_t x) { return !special80(x) && !x.signif; }
/* A finite nonzero x as a significand with bit 63 set and its exponent (subnormals normalised). */
static struct exp32_sig64 unpacked80(extFloat80_t x) {
  struct exp32_sig64 r = {x.signExp & 0x7FFF, x.signif};
  if (!r.exp) r.exp = 1;
  if (!(r.sig >> 63)) {
    struct exp32_sig64 n = softfloat_normSubnormalExtF80Sig(r.sig);
    r.exp += n.exp;
    r.sig = n.sig;
  }
  return r;
}
long double fmal(long double a, long double b, long double c) {
  extFloat80_t x = x80_of(a), y = x80_of(b), z = x80_of(c);
  if (special80(z) && !special80(x) && !special80(y)) return of_x80(extF80_add(extF80_add(z, x), y));
  if (zero80(z) && !zero80(x) && !zero80(y)) return of_x80(extF80_mul(x, y));
  if (special80(x) || special80(y) || special80(z) || zero80(x) || zero80(y))
    return of_x80(extF80_add(extF80_mul(x, y), z));
  struct exp32_sig64 ux = unpacked80(x), uy = unpacked80(y), uz = unpacked80(z);
  bool signP = (x.signExp ^ y.signExp) >> 15, signZ = z.signExp >> 15;
  /* A significand of 128 bits with bit 127 set and exponent e stands for sig * 2^(e - 0x3FFF - 127). */
  uti_t p = (uti_t)ux.sig * uy.sig, q = (uti_t)uz.sig << 64;
  int_fast32_t expP = ux.exp + uy.exp - 0x3FFE, expZ = uz.exp;
  if (!(p >> 127)) {
    p <<= 1;
    --expP;
  }
  bool pFirst = expP > expZ || (expP == expZ && p >= q);
  uti_t big = pFirst ? p : q, small = pFirst ? q : p;
  int_fast32_t exp = pFirst ? expP : expZ, shift = pFirst ? expP - expZ : expZ - expP;
  bool sign = pFirst ? signP : signZ;
  /* The window: high and low 128 bits. */
  uti_t smallHigh, smallLow;
  if (shift == 0) {
    smallHigh = small;
    smallLow = 0;
  } else if (shift < 128) {
    smallHigh = small >> shift;
    smallLow = small << (128 - shift);
  } else if (shift == 128) {
    smallHigh = 0;
    smallLow = small;
  } else if (shift < 256) {
    smallHigh = 0;
    smallLow = small >> (shift - 128) | (small << (256 - shift) != 0);
  } else {
    smallHigh = 0;
    smallLow = 1;
  }
  uti_t high, low;
  if (signP == signZ) {
    low = smallLow;
    high = big + smallHigh;
    if (high < big) { /* a carry out: one place right, keeping what falls off */
      low = low >> 1 | high << 127 | (low & 1);
      high = high >> 1 | (uti_t)1 << 127;
      ++exp;
    }
  } else {
    low = -smallLow;
    high = big - smallHigh - (smallLow != 0);
    if (!high && !low) return of_x80((extFloat80_t){0, 0}); /* exact cancellation: +0 to nearest */
    if (!high) {
      high = low;
      low = 0;
      exp -= 128;
    }
    uint64_t top = (uint64_t)(high >> 64);
    int_fast32_t n = top ? softfloat_countLeadingZeros64(top) : 64 + softfloat_countLeadingZeros64((uint64_t)high);
    if (n) {
      high = high << n | low >> (128 - n);
      low <<= n;
      exp -= n;
    }
  }
  return of_x80(softfloat_roundPackToExtF80(sign, exp, (uint64_t)(high >> 64), (uint64_t)high | (low != 0),
                                            extF80_roundingPrecision));
}

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
