/* The compiler runtime-call interface for floating point, over Berkeley SoftFloat 3e.
 *
 * bennett-front rewrites each floating-point operation into a call of one of these functions, as LLVM's
 * soft-float legalisation does for a machine without floating-point hardware: the libgcc / compiler-rt
 * names and signatures (`double __adddf3(double, double)`), and for what has no libgcc name the C
 * library names LLVM's own RuntimeLibcalls.def uses (`sqrt`, `fma`, `floor`, `fmin`, ...). Any bitcode
 * that defines these names can replace this file's (`bennett-front --float-runtime`).
 *
 * Every operation rounds in the current rounding mode and raises the exception flags of the IEEE operation,
 * with the NaNs of x86 SSE (the 8086-SSE specialisation): what the natively compiled program computes.
 *
 * Global state: the floating-point environment. SoftFloat keeps `softfloat_roundingMode`,
 * `softfloat_detectTininess`, `extF80_roundingPrecision` and `softfloat_exceptionFlags` in globals
 * (softfloat_state.c), which are ordinary memory of the program once bennett-front has linked the runtime.
 * The C library's <fenv.h> (rt/musl/fenv.c) reads and writes the rounding mode and the flags through the four
 * `__bennett_fe*` functions at the end of this file, in x86's encodings. A program that never names a <fenv.h>
 * function never links them. Then nothing writes the mode, and LLVM's optimiser folds its loads to the initial
 * value (to nearest, ties to even); nothing reads the flags, and bennett-front deletes every write to them
 * (LLVM's optimiser would not); so no routine writes memory. No routine computes a result from the flags,
 * which makes that deletion exact. The tininess rule (after rounding, as x86 detects it) and the 80-bit
 * rounding precision (x87's on Linux) are never written.
 */

#include <stdbool.h>
#include <stdint.h>

#include "platform.h"
#include "softfloat.h"
#include "specialize.h" /* the default NaN */
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
static extFloat80_t x80_bits(long double x) {
  extFloat80_t r;
  __builtin_memcpy(&r.signif, &x, 8);
  __builtin_memcpy(&r.signExp, (const char *)&x + 8, 2);
  return r;
}

/* x87 checks every operand's encoding before it computes (Intel SDM vol. 1, 8.2.2, unsupported encodings
 * and pseudo-denormals), and so does every operation here, through `x80_operand`. SoftFloat takes encodings
 * at face value: it orders a pseudo-denormal below the smallest normal and computes with an unnormal as with
 * a number. No arithmetic produces these encodings; only a write of the bytes does.
 *
 * An unsupported encoding (an exponent other than 0 with the integer bit clear: an unnormal, an unnormal
 * zero, a pseudo-infinity, a pseudo-NaN) is an invalid operand: the invalid flag is raised and the result is
 * the default NaN, x87's indefinite, even beside a NaN operand. */
static const extFloat80_t indefinite80 = {.signif = defaultNaNExtF80UI0, .signExp = defaultNaNExtF80UI64};
static bool unsupported80(extFloat80_t x) { return (x.signExp & 0x7FFF) && !(x.signif >> 63); }

/* Whether x87 reads `x` otherwise than SoftFloat would: an unsupported encoding, or a pseudo-denormal
 * (exponent 0, integer bit set). Exactly the encodings whose integer bit equals "the exponent is 0"; one
 * test, so that canonical operands, all that arithmetic produces, pay only for it. */
static bool noncanonical80(extFloat80_t x) { return (x.signif >> 63) == !(x.signExp & 0x7FFF); }

/* A noncanonical operand as x87 reads it, `*invalid` set if it is an unsupported encoding (which reads as
 * the indefinite). A pseudo-denormal reads as the denormal of the same value: the normal number with
 * exponent 1 and the same significand. */
static extFloat80_t x80_canonical(extFloat80_t r, bool *invalid) {
  if (unsupported80(r)) {
    softfloat_raiseFlags(softfloat_flag_invalid);
    *invalid = true;
    return indefinite80;
  }
  r.signExp |= 1;
  return r;
}
static extFloat80_t x80_operand(long double x, bool *invalid) {
  extFloat80_t r = x80_bits(x);
  return __builtin_expect(noncanonical80(r), 0) ? x80_canonical(r, invalid) : r;
}
/* One operand, where the indefinite it reads as for an unsupported encoding is already the result. */
static extFloat80_t x80_of(long double x) {
  bool invalid = false;
  return x80_operand(x, &invalid);
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

void softfloat_raiseFlags(uint_fast8_t flags) { softfloat_exceptionFlags |= flags; }

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
static long double arith80(extFloat80_t (*op)(extFloat80_t, extFloat80_t), long double a, long double b) {
  bool invalid = false;
  extFloat80_t x = x80_operand(a, &invalid), y = x80_operand(b, &invalid);
  return of_x80(invalid ? indefinite80 : op(x, y));
}
long double __addxf3(long double a, long double b) { return arith80(extF80_add, a, b); }
long double __subxf3(long double a, long double b) { return arith80(extF80_sub, a, b); }
long double __mulxf3(long double a, long double b) { return arith80(extF80_mul, a, b); }
long double __divxf3(long double a, long double b) { return arith80(extF80_div, a, b); }

/* Comparisons, as libgcc defines them: eq and ne return 0 exactly when a == b; lt and le return a
 * negative number, 0 or a positive number as a < b, a == b or a > b, and 1 when either is a NaN; gt and
 * ge the same, but -1 for a NaN; unord a nonzero number when either is a NaN. The quiet comparisons
 * (x86's ucomisd and fucomi, which clang uses for a plain `fcmp`): invalid is raised for a signalling NaN only. */
static bool quiet_unordered32(float32_t a, float32_t b) {
  if (softfloat_isSigNaNF32UI(a.v) || softfloat_isSigNaNF32UI(b.v)) softfloat_raiseFlags(softfloat_flag_invalid);
  return nan32(a) || nan32(b);
}
static bool quiet_unordered64(float64_t a, float64_t b) {
  if (softfloat_isSigNaNF64UI(a.v) || softfloat_isSigNaNF64UI(b.v)) softfloat_raiseFlags(softfloat_flag_invalid);
  return nan64(a) || nan64(b);
}
static bool quiet_unordered80(extFloat80_t a, extFloat80_t b) {
  if (softfloat_isSigNaNExtF80UI(a.signExp, a.signif) || softfloat_isSigNaNExtF80UI(b.signExp, b.signif))
    softfloat_raiseFlags(softfloat_flag_invalid);
  return nan80(a) || nan80(b);
}
static cmp_t order32(float32_t a, float32_t b, cmp_t unordered) {
  if (quiet_unordered32(a, b)) return unordered;
  return f32_lt_quiet(a, b) ? -1 : f32_eq(a, b) ? 0 : 1;
}
static cmp_t order64(float64_t a, float64_t b, cmp_t unordered) {
  if (quiet_unordered64(a, b)) return unordered;
  return f64_lt_quiet(a, b) ? -1 : f64_eq(a, b) ? 0 : 1;
}
static cmp_t order80(extFloat80_t a, extFloat80_t b, cmp_t unordered) {
  if (quiet_unordered80(a, b)) return unordered;
  return extF80_lt_quiet(a, b) ? -1 : extF80_eq(a, b) ? 0 : 1;
}
cmp_t __eqsf2(float a, float b) { return !f32_eq(f32_of(a), f32_of(b)); }
cmp_t __nesf2(float a, float b) { return !f32_eq(f32_of(a), f32_of(b)); }
cmp_t __ltsf2(float a, float b) { return order32(f32_of(a), f32_of(b), 1); }
cmp_t __lesf2(float a, float b) { return order32(f32_of(a), f32_of(b), 1); }
cmp_t __gtsf2(float a, float b) { return order32(f32_of(a), f32_of(b), -1); }
cmp_t __gesf2(float a, float b) { return order32(f32_of(a), f32_of(b), -1); }
cmp_t __unordsf2(float a, float b) { return quiet_unordered32(f32_of(a), f32_of(b)); }
cmp_t __eqdf2(double a, double b) { return !f64_eq(f64_of(a), f64_of(b)); }
cmp_t __nedf2(double a, double b) { return !f64_eq(f64_of(a), f64_of(b)); }
cmp_t __ltdf2(double a, double b) { return order64(f64_of(a), f64_of(b), 1); }
cmp_t __ledf2(double a, double b) { return order64(f64_of(a), f64_of(b), 1); }
cmp_t __gtdf2(double a, double b) { return order64(f64_of(a), f64_of(b), -1); }
cmp_t __gedf2(double a, double b) { return order64(f64_of(a), f64_of(b), -1); }
cmp_t __unorddf2(double a, double b) { return quiet_unordered64(f64_of(a), f64_of(b)); }
cmp_t __eqxf2(long double a, long double b) { return !extF80_eq(x80_of(a), x80_of(b)); }
cmp_t __nexf2(long double a, long double b) { return !extF80_eq(x80_of(a), x80_of(b)); }
cmp_t __ltxf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), 1); }
cmp_t __lexf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), 1); }
cmp_t __gtxf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), -1); }
cmp_t __gexf2(long double a, long double b) { return order80(x80_of(a), x80_of(b), -1); }
cmp_t __unordxf2(long double a, long double b) { return quiet_unordered80(x80_of(a), x80_of(b)); }
/* A signalling comparison (`llvm.experimental.constrained.fcmps`: C's <, <=, >, >= under FENV_ACCESS, x86's
 * comisd and fcomi) raises invalid for any NaN operand, a quiet one only for a signalling NaN; bennett-front
 * calls this beside the quiet comparison. */
void __bennett_signalsf2(float a, float b) {
  if (nan32(f32_of(a)) || nan32(f32_of(b))) softfloat_raiseFlags(softfloat_flag_invalid);
}
void __bennett_signaldf2(double a, double b) {
  if (nan64(f64_of(a)) || nan64(f64_of(b))) softfloat_raiseFlags(softfloat_flag_invalid);
}
void __bennett_signalxf2(long double a, long double b) {
  if (nan80(x80_of(a)) || nan80(x80_of(b))) softfloat_raiseFlags(softfloat_flag_invalid);
}

/* Conversions. To an integer: rounded toward zero, as C converts, raising inexact when that changes the value
 * (cvttsd2si and fistp do) and invalid out of range and for a NaN. Out of range is undefined in C; there the
 * signed conversions give what x86 SSE gives (the 8086-SSE specialisation's value), and the unsigned ones
 * what clang's x86-64 code gives for the same source: to 32 bits, the low half of the signed 64-bit
 * conversion; to 64 bits, the signed conversion below 2^63 (so a negative value wraps), 2^63 from 2^64 up
 * and for a NaN. For long double, x87 (`fistp`) gives the same: the indefinite integer, the least of the
 * width, out of range, and to 64 bits unsigned, the signed conversion below 2^63 and that of x - 2^63
 * with its top bit flipped from there on, which is 0 from 2^64 up and for a NaN. Beside invalid, these raise no
 * inexact (clang's unsigned 64-bit sequence can, from its subtraction of 2^63). */
double __extendsfdf2(float a) { return of_f64(f32_to_f64(f32_of(a))); }
float __truncdfsf2(double a) { return of_f32(f64_to_f32(f64_of(a))); }
int __fixsfsi(float a) { return f32_to_i32_r_minMag(f32_of(a), true); }
long long __fixsfdi(float a) { return f32_to_i64_r_minMag(f32_of(a), true); }
unsigned __fixunssfsi(float a) { return (unsigned)f32_to_i64_r_minMag(f32_of(a), true); }
unsigned long long __fixunssfdi(float a) {
  float32_t x = f32_of(a), two63 = {0x5F000000}, two64 = {0x5F800000};
  if (f32_lt_quiet(x, two63)) return (unsigned long long)f32_to_i64_r_minMag(x, true);
  if (f32_lt_quiet(x, two64)) return f32_to_ui64_r_minMag(x, true);
  softfloat_raiseFlags(softfloat_flag_invalid);
  return UINT64_C(0x8000000000000000);
}
int __fixdfsi(double a) { return f64_to_i32_r_minMag(f64_of(a), true); }
long long __fixdfdi(double a) { return f64_to_i64_r_minMag(f64_of(a), true); }
unsigned __fixunsdfsi(double a) { return (unsigned)f64_to_i64_r_minMag(f64_of(a), true); }
unsigned long long __fixunsdfdi(double a) {
  float64_t x = f64_of(a), two63 = {UINT64_C(0x43E0000000000000)}, two64 = {UINT64_C(0x43F0000000000000)};
  if (f64_lt_quiet(x, two63)) return (unsigned long long)f64_to_i64_r_minMag(x, true);
  if (f64_lt_quiet(x, two64)) return f64_to_ui64_r_minMag(x, true);
  softfloat_raiseFlags(softfloat_flag_invalid);
  return UINT64_C(0x8000000000000000);
}
long double __extendsfxf2(float a) { return of_x80(f32_to_extF80(f32_of(a))); }
long double __extenddfxf2(double a) { return of_x80(f64_to_extF80(f64_of(a))); }
float __truncxfsf2(long double a) { return of_f32(extF80_to_f32(x80_of(a))); }
double __truncxfdf2(long double a) { return of_f64(extF80_to_f64(x80_of(a))); }
int __fixxfsi(long double a) { return extF80_to_i32_r_minMag(x80_of(a), true); }
/* x87's 16-bit conversion (`fistp` to a word), which LLVM's x86-64 back end uses from long double to an
 * integer of 16 bits or fewer: outside the 16-bit range the indefinite integer -2^15 and only invalid. */
int __bennett_fixxfhi(long double a) {
  uint_fast8_t before = softfloat_exceptionFlags;
  int_fast32_t r = extF80_to_i32_r_minMag(x80_of(a), true);
  if (r < -0x8000 || r > 0x7FFF) {
    softfloat_exceptionFlags = before | softfloat_flag_invalid;
    return -0x8000;
  }
  return (int)r;
}
long long __fixxfdi(long double a) { return extF80_to_i64_r_minMag(x80_of(a), true); }
unsigned __fixunsxfsi(long double a) { return (unsigned)extF80_to_i64_r_minMag(x80_of(a), true); }
unsigned long long __fixunsxfdi(long double a) {
  extFloat80_t x = x80_of(a), two63 = {UINT64_C(0x8000000000000000), 0x403E},
               two64 = {UINT64_C(0x8000000000000000), 0x403F};
  if (extF80_lt_quiet(x, two63)) return (unsigned long long)extF80_to_i64_r_minMag(x, true);
  if (extF80_lt_quiet(x, two64)) return extF80_to_ui64_r_minMag(x, true);
  softfloat_raiseFlags(softfloat_flag_invalid);
  return 0;
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
  if (f64_le_quiet(two63, x)) return (uint64_t)f64_to_i64_r_minMag(f64_sub(x, two63), true) ^ UINT64_C(0x8000000000000000);
  return (uint64_t)f64_to_i64_r_minMag(x, true);
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
  if (extF80_le_quiet(two63, x)) return (uint64_t)extF80_to_i64_r_minMag(extF80_sub(x, two63), true) ^ UINT64_C(0x8000000000000000);
  return (uint64_t)extF80_to_i64_r_minMag(x, true);
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
 * half-precision hardware: every operation in float, rounded back after each, through these conversions,
 * which are libgcc's soft-fp routines natively: they round in the current mode and raise the IEEE flags,
 * detecting tininess after rounding, as x86. For _Float16 they are SoftFloat's (one rounding each);
 * __fixhfti and friends follow soft-fp, which saturates: an infinity or a NaN gives the largest or least
 * integer by its sign and invalid; unsigned, a value in (-1, 0) gives 0 and inexact, one of -1 or less 0 and
 * invalid. A _Float16 never needs more than 17 integer bits, so the 64-bit conversions do the rest; a 128-bit
 * integer outside 64 bits overflows, as the largest 64-bit one of its sign does. */
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
  if ((x.v & 0x7C00) == 0x7C00) {
    softfloat_raiseFlags(softfloat_flag_invalid);
    return x.v >> 15 ? (ti_t)((uti_t)1 << 127) : (ti_t)(~(uti_t)0 >> 1);
  }
  return f16_to_i64_r_minMag(x, true);
}
uti_t __fixunshfti(_Float16 a) {
  float16_t x = f16_of(a);
  if (x.v >> 15) {
    if (x.v & 0x7FFF) softfloat_raiseFlags((x.v & 0x7FFF) < 0x3C00 ? softfloat_flag_inexact : softfloat_flag_invalid);
    return 0;
  }
  if ((x.v & 0x7C00) == 0x7C00) {
    softfloat_raiseFlags(softfloat_flag_invalid);
    return ~(uti_t)0;
  }
  return f16_to_ui64_r_minMag(x, true);
}
_Float16 __floattihf(ti_t a) {
  return of_f16(i64_to_f16(a == (int64_t)a ? (int64_t)a : a < 0 ? INT64_MIN : INT64_MAX));
}
_Float16 __floatuntihf(uti_t a) { return of_f16(ui64_to_f16(a >> 64 ? UINT64_MAX : (uint64_t)a)); }

/* __bf16 is the top half of a float, which is all an extension does (the front writes it inline). SoftFloat
 * has no bfloat16, so the roundings to it are written here, as libgcc's soft-fp computes them: from float, the
 * low 16 bits rounded off in the current mode (a carry runs into the exponent, which is right up to infinity),
 * inexact when they are not 0, overflow when the result is infinite, underflow when it is also tiny after
 * rounding (below 2^-126 even with an unbounded exponent); a NaN quiet with the top of its payload, invalid for
 * a signalling one. From double and long double: rounded to float to odd (a float that is inexact is made odd
 * towards the exact value), then to __bf16, which is one correct rounding because float keeps at least two
 * bits more than __bf16 everywhere, subnormals included (the exponent ranges are the same); only the invalid
 * flag of the first rounding is kept. A magnitude of 2^128 or more overflows in every mode, which the float
 * between cannot show (it is the largest float, odd), so it is decided first. */
static bool rounds_up(uint32_t low, uint32_t half, bool odd, bool sign) {
  switch (softfloat_roundingMode) {
  case softfloat_round_near_even:
    return low > half || (low == half && odd);
  case softfloat_round_near_maxMag:
    return low >= half;
  case softfloat_round_min:
    return sign && low;
  case softfloat_round_max:
    return !sign && low;
  default: /* softfloat_round_minMag: no other mode is ever set */
    return false;
  }
}
static __bf16 of_bf16(uint16_t r) {
  __bf16 b;
  __builtin_memcpy(&b, &r, sizeof b);
  return b;
}
static __bf16 bf16_of_bits(uint32_t u) {
  uint32_t m = u & 0x7FFFFFFF;
  bool sign = u >> 31;
  if (m > 0x7F800000) {
    if (!(u & 0x00400000)) softfloat_raiseFlags(softfloat_flag_invalid);
    return of_bf16((uint16_t)(u >> 16 | 0x0040));
  }
  uint32_t low = m & 0xFFFF;
  uint16_t magnitude = (uint16_t)((m >> 16) + rounds_up(low, 0x8000, m >> 16 & 1, sign));
  if (low) {
    uint_fast8_t flags = softfloat_flag_inexact;
    if (magnitude == 0x7F80) flags |= softfloat_flag_overflow;
    if (m < 0x00800000 && !((m >> 15) == 0xFF && rounds_up(m & 0x7FFF, 0x4000, true, sign)))
      flags |= softfloat_flag_underflow;
    softfloat_raiseFlags(flags);
  }
  return of_bf16((uint16_t)(sign << 15 | magnitude));
}
static __bf16 bf16_overflow(bool sign) {
  softfloat_raiseFlags(softfloat_flag_overflow | softfloat_flag_inexact);
  bool infinite = rounds_up(1, 0, false, sign); /* away from zero by any amount */
  return of_bf16((uint16_t)(sign << 15 | (infinite ? 0x7F80 : 0x7F7F)));
}
static uint32_t odd_step(float32_t r, bool inexact, bool below) {
  return inexact && !(r.v & 1) ? (below ? r.v + 1 : r.v - 1) : r.v;
}
__bf16 __truncsfbf2(float a) { return bf16_of_bits(f32_of(a).v); }
__bf16 __truncdfbf2(double a) {
  float64_t x = f64_of(a), ax = {x.v & UINT64_C(0x7FFFFFFFFFFFFFFF)};
  if (ax.v >= UINT64_C(0x47F0000000000000) && ax.v < UINT64_C(0x7FF0000000000000)) return bf16_overflow(x.v >> 63);
  uint_fast8_t before = softfloat_exceptionFlags;
  float32_t r = f64_to_f32(x);
  float64_t back = f32_to_f64(r), ab = {back.v & UINT64_C(0x7FFFFFFFFFFFFFFF)};
  uint32_t odd = odd_step(r, !nan64(x) && !f64_eq(back, x), f64_lt_quiet(ab, ax));
  softfloat_exceptionFlags = before | (softfloat_exceptionFlags & softfloat_flag_invalid);
  return bf16_of_bits(odd);
}
__bf16 __truncxfbf2(long double a) {
  extFloat80_t x = x80_of(a), ax = {x.signif, (uint16_t)(x.signExp & 0x7FFF)};
  if (ax.signExp >= 0x407F && ax.signExp < 0x7FFF) return bf16_overflow(x.signExp >> 15);
  uint_fast8_t before = softfloat_exceptionFlags;
  float32_t r = extF80_to_f32(x);
  extFloat80_t back = f32_to_extF80(r), ab = {back.signif, (uint16_t)(back.signExp & 0x7FFF)};
  uint32_t odd = odd_step(r, !nan80(x) && !extF80_eq(back, x), extF80_lt_quiet(ab, ax));
  softfloat_exceptionFlags = before | (softfloat_exceptionFlags & softfloat_flag_invalid);
  return bf16_of_bits(odd);
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

/* libgcc2.c starts from `y = n % 2 ? x : 1`; for m == 1 the loop does not run and x itself is returned,
 * with no arithmetic to read it, so a noncanonical encoding comes back unchanged. On every other path x or
 * y meets a multiplication or the division, which reads it as x87 does, so reading x once at entry gives
 * the same results. */
long double __powixf2(long double x, int m) {
  if (m == 1) return x;
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
/* fmaf and fma: x86's FMA instructions, which glibc's use. A NaN addend beside factors that are not NaNs is the
 * result, quieted, and only a signalling one raises invalid, also when the product is zero times infinity
 * (SoftFloat gives the default NaN and raises invalid there). Otherwise SoftFloat's, NaNs in the order of the
 * operands as x86's. */
float fmaf(float a, float b, float c) {
  float32_t x = f32_of(a), y = f32_of(b), z = f32_of(c);
  if (nan32(z) && !nan32(x) && !nan32(y)) {
    if (softfloat_isSigNaNF32UI(z.v)) softfloat_raiseFlags(softfloat_flag_invalid);
    return of_f32((float32_t){z.v | 0x00400000});
  }
  return of_f32(f32_mulAdd(x, y, z));
}
double fma(double a, double b, double c) {
  float64_t x = f64_of(a), y = f64_of(b), z = f64_of(c);
  if (nan64(z) && !nan64(x) && !nan64(y)) {
    if (softfloat_isSigNaNF64UI(z.v)) softfloat_raiseFlags(softfloat_flag_invalid);
    return of_f64((float64_t){z.v | UINT64_C(0x0008000000000000)});
  }
  return of_f64(f64_mulAdd(x, y, z));
}
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
    /* exact cancellation: +0, but -0 rounding downward (IEEE 754, 6.3) */
    if (!high && !low) {
      return of_x80((extFloat80_t){0, (uint16_t)(softfloat_roundingMode == softfloat_round_min ? 0x8000 : 0)});
    }
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

/* Rounding to an integral value. rint and nearbyint round in the current mode, rint raising inexact when the
 * value changes (x87's frndint, SSE4.1's roundsd, which glibc's use), nearbyint not; the others round in
 * their own mode and never raise inexact (glibc's roundsd with the inexact exception suppressed, its x87 code
 * restoring the flags, or its integer operations). A signalling NaN raises invalid in each. */
#define ROUND_SSE(name, mode, exact)                                                                       \
  float name##f(float a) { return of_f32(f32_roundToInt(f32_of(a), mode, exact)); }                      \
  double name(double a) { return of_f64(f64_roundToInt(f64_of(a), mode, exact)); }
#define ROUND(name, mode, exact)                                                                           \
  ROUND_SSE(name, mode, exact)                                                                             \
  long double name##l(long double a) { return of_x80(extF80_roundToInt(x80_of(a), mode, exact)); }
ROUND(floor, softfloat_round_min, false)
ROUND(ceil, softfloat_round_max, false)
ROUND(trunc, softfloat_round_minMag, false)
ROUND_SSE(round, softfloat_round_near_maxMag, false)
ROUND_SSE(roundeven, softfloat_round_near_even, false)
ROUND(rint, softfloat_roundingMode, true)
ROUND(nearbyint, softfloat_roundingMode, false)

/* roundevenl. The native program calls glibc's, which is software (sysdeps/ieee754/ldbl-96), not x87's
 * frndint: it rounds the significand as an integer at the bit where the exponent puts the binary point and
 * clears the bits below, and it never reads the integer bit, so it takes an encoding no arithmetic produces
 * at face value. Below the largest exponent this follows it bit for bit (measured against glibc on every
 * sign and exponent: `front/tests/c/p0_x87_matrix.c`, mode `rounding`). From exponent 0x3FFF + 63 the
 * value is returned. From 0x3FFF the significand is rounded to even in place, so an unnormal stays an
 * unnormal unless a carry reaches the integer bit (glibc's carry out of the significand, into the exponent,
 * cannot happen without the integer bit and is left out). At 0x3FFE the result is 1 (signed) when the high
 * 32 bits of the significand exceed 0x80000000 or the low 32 bits are not 0, glibc's test for the interval
 * (0.5, 1), which an unnormal can pass. Below, a zero of the same sign. At the largest exponent glibc returns x + x, an x87 addition, which
 * reads a pseudo-infinity or pseudo-NaN as an invalid operand: the indefinite, as `x80_of` gives.
 * Canonical operands take SoftFloat's rounding, which gives the same results on them. */
static extFloat80_t roundeven_face_value(extFloat80_t x) {
  uint_fast16_t exponent = x.signExp & 0x7FFF;
  uint16_t sign = x.signExp & 0x8000;
  if (exponent >= 0x3FFF + 63) return x;
  if (exponent >= 0x3FFF) {
    uint64_t one = UINT64_C(1) << (0x3FFF + 63 - exponent), half = one >> 1;
    if (x.signif & (one | (half - 1))) x.signif += half;
    x.signif &= ~(one - 1);
    return x;
  }
  if (exponent == 0x3FFE && ((x.signif >> 32) > 0x80000000 || (uint32_t)x.signif)) {
    return (extFloat80_t){.signif = UINT64_C(0x8000000000000000), .signExp = (uint16_t)(sign | 0x3FFF)};
  }
  return (extFloat80_t){.signif = 0, .signExp = sign};
}
long double roundevenl(long double a) {
  extFloat80_t x = x80_bits(a);
  if (__builtin_expect(noncanonical80(x), 0) && (x.signExp & 0x7FFF) != 0x7FFF) {
    return of_x80(roundeven_face_value(x));
  }
  return of_x80(extF80_roundToInt(x80_of(a), softfloat_round_near_even, false));
}

/* roundl. glibc's is software as well (ldbl-96), with its own bit operations on the two 32-bit halves of the
 * significand, which also never read the integer bit; this follows them bit for bit below the largest
 * exponent (measured on every sign and exponent: `front/tests/c/p0_x87_matrix.c`, mode `rounding`). Below
 * exponent 0x3FFE the result is a zero of the same sign, at 0x3FFE it is 1 (signed) whatever the
 * significand. From 0x3FFF to 0x3FFF + 30 an integral value is returned as it is; otherwise half is added
 * at the place of the binary point, the bits below it are cleared and the integer bit is set. From
 * 0x3FFF + 31 to 0x3FFF + 62 the same is done in the low half, with a carry into the high half, and the
 * integer bit is left as it is. Above, the value is returned. glibc's carries into the exponent cannot happen
 * without the integer bit and are left out. */
static extFloat80_t round_face_value(extFloat80_t x) {
  int_fast32_t exponent = (int_fast32_t)(x.signExp & 0x7FFF) - 0x3FFF;
  uint32_t high = (uint32_t)(x.signif >> 32), low = (uint32_t)x.signif;
  if (exponent > 62) return x;
  if (exponent < 0) {
    bool one = exponent == -1;
    return (extFloat80_t){.signif = one ? UINT64_C(0x8000000000000000) : 0,
                          .signExp = (uint16_t)((x.signExp & 0x8000) | (one ? 0x3FFF : 0))};
  }
  if (exponent < 31) {
    uint32_t fraction = UINT32_C(0x7FFFFFFF) >> exponent;
    if (!((high & fraction) | low)) return x;
    high = ((high + (UINT32_C(0x40000000) >> exponent)) & ~fraction) | UINT32_C(0x80000000);
    low = 0;
  } else {
    uint32_t fraction = UINT32_C(0xFFFFFFFF) >> (exponent - 31);
    if (!(low & fraction)) return x;
    uint32_t sum = low + (UINT32_C(1) << (62 - exponent));
    if (sum < low) ++high;
    low = sum & ~fraction;
  }
  x.signif = (uint64_t)high << 32 | low;
  return x;
}
long double roundl(long double a) {
  extFloat80_t x = x80_bits(a);
  if (__builtin_expect(noncanonical80(x), 0) && (x.signExp & 0x7FFF) != 0x7FFF) {
    return of_x80(round_face_value(x));
  }
  return of_x80(extF80_roundToInt(x80_of(a), softfloat_round_near_maxMag, false));
}

/* lroundl and llroundl (`llvm.lround`, `llvm.llround` on long double, and the library calls). glibc's are
 * software (sysdeps/ieee754/ldbl-96, s_llroundl.c and s_lroundl.c, the same on x86-64 where long is 64 bits):
 * integer operations on the two 32-bit halves of the significand that never read the integer bit, so they take
 * an encoding no arithmetic produces at face value (an unnormal zero at exponent 0x3FFF rounds to 0, where
 * roundl followed by a conversion, which reads it as x87 does, gives the integer indefinite). This follows them
 * bit for bit (measured against glibc on every sign and exponent: `front/tests/c/p0_x87_matrix.c`, mode
 * `rounding`): below exponent 0x3FFE the result is 0, at 0x3FFE it is 1 (signed) whatever the significand;
 * up to 0x3FFF + 30 half is added at the binary point of the high half (a carry out of it shifts in a set
 * integer bit and moves the point one place) and the integral part taken; up to 0x3FFF + 62 the same in the
 * low half, with a carry into the high half. From 0x3FFF + 63 (the integer is too large, or the operand is an
 * infinity or a NaN) glibc converts the operand itself as x87 does (`(long long int) x`), which `__fixxfdi`
 * gives. The sign is applied last, wrapping. */
static long long round_to_integer(long double a) {
  extFloat80_t x = x80_bits(a);
  int_fast32_t exponent = (int_fast32_t)(x.signExp & 0x7FFF) - 0x3FFF;
  uint32_t high = (uint32_t)(x.signif >> 32), low = (uint32_t)x.signif;
  uint64_t result;
  if (exponent < 0) {
    result = exponent == -1;
  } else if (exponent < 31) {
    uint32_t sum = high + (UINT32_C(0x40000000) >> exponent);
    if (sum < high) {
      sum = sum >> 1 | UINT32_C(0x80000000);
      ++exponent;
    }
    result = sum >> (31 - exponent);
  } else if (exponent < 63) {
    uint32_t sum = low + (UINT32_C(0x80000000) >> (exponent - 31));
    uint64_t integral = high;
    if (sum < low) ++integral;
    result = exponent == 31 ? integral : integral << (exponent - 31) | sum >> (63 - exponent);
  } else {
    return __fixxfdi(a);
  }
  return (long long)((x.signExp & 0x8000) ? -result : result);
}
long lroundl(long double a) { return round_to_integer(a); }
long long llroundl(long double a) { return round_to_integer(a); }

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

/* glibc's fmin, fmax, fminf and fmaxf (sysdeps/x86_64/fpu/s_fmin.S and its siblings), which the native program
 * calls for `llvm.experimental.constrained.minnum` and `maxnum` (LLVM's x86-64 back end has no strict inline
 * form): of ordered operands minsd's or maxsd's choice, the second of equal ones; a quiet NaN beside a number
 * gives the number; a signalling NaN or two NaNs give their sum (the first NaN, quieted; invalid for a signalling
 * one). */
#define GLIBC_MINMAX(T, name, of, F, nan, signalling, before, add)                                         \
  T name(T x, T y) {                                                                                       \
    F a = of(x), b = of(y);                                                                                \
    if (!nan(a) && !nan(b)) return before ? x : y;                                                         \
    if (!nan(b) && !signalling(a.v)) return y;                                                             \
    if (!nan(a) && !signalling(b.v)) return x;                                                             \
    return add(x, y);                                                                                      \
  }
GLIBC_MINMAX(float, __bennett_fminf, f32_of, float32_t, nan32, softfloat_isSigNaNF32UI, f32_lt_quiet(a, b), __addsf3)
GLIBC_MINMAX(float, __bennett_fmaxf, f32_of, float32_t, nan32, softfloat_isSigNaNF32UI, f32_lt_quiet(b, a), __addsf3)
GLIBC_MINMAX(double, __bennett_fmin, f64_of, float64_t, nan64, softfloat_isSigNaNF64UI, f64_lt_quiet(a, b), __adddf3)
GLIBC_MINMAX(double, __bennett_fmax, f64_of, float64_t, nan64, softfloat_isSigNaNF64UI, f64_lt_quiet(b, a), __adddf3)

/* fminl and fmaxl: on x86-64 the native program calls the C library's (LLVM has no inline expansion for
 * long double), and these are glibc's (sysdeps/x86_64/fpu/s_fminl.S and s_fmaxl.S, x87 code): of equal
 * operands, fminl gives the first and fmaxl the second. When the operands are unordered (a NaN or an
 * unsupported encoding, which x87 compares as unordered), glibc's code gives the other operand, unchanged,
 * if exactly one is unordered with itself and bit 62 of that one's significand is set (a quiet NaN's quiet
 * bit, read from memory, so also a pseudo-NaN's or an unnormal's), and otherwise their sum: a quiet NaN by
 * x87's rules, the indefinite for an unsupported encoding. */
static long double unordered80(long double x, long double y) {
  bool x_quiet_bit = x80_bits(x).signif >> 62 & 1, y_quiet_bit = x80_bits(y).signif >> 62 & 1;
  if (!nan80(x80_of(y))) return x_quiet_bit ? y : __addxf3(x, y);
  if (!nan80(x80_of(x))) return y_quiet_bit ? x : __addxf3(x, y);
  return __addxf3(x, y);
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

/* The floating-point environment, for the C library's <fenv.h> (rt/musl/fenv.c), in x86's encodings: a
 * rounding mode is the RC field of the x87 control word (to nearest 0, downward 0x400, upward 0x800, toward
 * zero 0xC00), and the flags are the x87 status word's and MXCSR's (invalid 1, divide by zero 4, overflow 8,
 * underflow 16, inexact 32). x86 has one mode for SSE and one for x87, which glibc's fesetround sets alike,
 * and one set of flags in each unit, which its fetestexcept reads together: here they are one mode and one
 * set of flags. bennett-front links these like the routines above, also into a program with no other
 * floating point. */
int __bennett_fegetround(void) {
  switch (softfloat_roundingMode) {
  case softfloat_round_min:
    return 0x400;
  case softfloat_round_max:
    return 0x800;
  case softfloat_round_minMag:
    return 0xC00;
  default: /* softfloat_round_near_even: no other mode is ever set */
    return 0;
  }
}
/* `mode` is one of the four (the C library checks). */
void __bennett_fesetround(int mode) {
  softfloat_roundingMode = mode == 0x400   ? softfloat_round_min
                           : mode == 0x800 ? softfloat_round_max
                           : mode == 0xC00 ? softfloat_round_minMag
                                           : softfloat_round_near_even;
}
int __bennett_fegetexcept(void) {
  uint_fast8_t f = softfloat_exceptionFlags;
  return (f & softfloat_flag_invalid ? 1 : 0) | (f & softfloat_flag_infinite ? 4 : 0) |
         (f & softfloat_flag_overflow ? 8 : 0) | (f & softfloat_flag_underflow ? 16 : 0) |
         (f & softfloat_flag_inexact ? 32 : 0);
}
void __bennett_fesetexcept(int flags) {
  softfloat_exceptionFlags = (flags & 1 ? softfloat_flag_invalid : 0) | (flags & 4 ? softfloat_flag_infinite : 0) |
                             (flags & 8 ? softfloat_flag_overflow : 0) | (flags & 16 ? softfloat_flag_underflow : 0) |
                             (flags & 32 ? softfloat_flag_inexact : 0);
}
