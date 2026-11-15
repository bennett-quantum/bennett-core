/* Berkeley SoftFloat 3e (8086-SSE specialisation, round to nearest even, tininess after rounding) against
 * this machine's floating point, bit for bit: SSE for float and double, x87 (default precision control:
 * 64-bit significand, round to nearest) for long double, the FMA instruction (vfmadd132, a * b + c) for
 * fma, and glibc for remainder and for the rounding functions that the x86-64 baseline lowers to library
 * calls (trunc, floor, ceil, round, roundeven, rint, lrint, lround).
 *
 * Usage: softfloat_exact [random-per-op [threads [exhaustive [name-filter]]]]
 *   The defaults (a fixed-seed sample, one thread, no exhaustive pass) are the regression test run by
 *   front/tests/tests/softfloat.rs; larger arguments are the long measurement.
 *
 * Every operation is checked on the directed cross product, then on random operands from mixed
 * generators. A mismatch is classified as `value` (some result not a NaN, or not both NaN), `nan-bits`
 * (both NaN, different bits), and either may be `noncanonical` (an extF80 operand that x87 does not
 * accept: unnormal, pseudo-denormal, pseudo-infinity, pseudo-NaN). Each class is either expected (a
 * known, explained difference listed in `expected`) or not; the exit status is the number of
 * unexpected classes, capped at 100. To-integer conversions of values outside the target range (and of
 * NaN) are undefined in C: they are counted apart as `out-of-range`, never as a failure. */

#include <math.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define LITTLEENDIAN 1 /* as SoftFloat's platform.h: extFloat80_t is {signif, signExp} */
#include "softfloat.h"

#define NOINLINE __attribute__((noinline))

typedef struct {
    uint64_t lo; /* f32: the low 32 bits; f64: all; extF80: the significand */
    uint16_t hi; /* extF80: sign and exponent */
} V;

enum fmt { F32, F64, X80, BOOL, I32, I64, U32, U64 };
static const char *const fmt_names[] = {"f32", "f64", "f80", "bool", "i32", "i64", "u32", "u64"};

/* ---- representations ---------------------------------------------------------------------------- */

static inline float hf32(V v) { uint32_t u = (uint32_t)v.lo; float f; memcpy(&f, &u, 4); return f; }
static inline double hf64(V v) { double d; memcpy(&d, &v.lo, 8); return d; }
static inline long double hx80(V v) {
    unsigned char b[16] = {0};
    memcpy(b, &v.lo, 8);
    memcpy(b + 8, &v.hi, 2);
    long double x;
    memcpy(&x, b, 16);
    return x;
}
static inline V vf32(float f) { uint32_t u; memcpy(&u, &f, 4); return (V){u, 0}; }
static inline V vf64(double d) { V v = {0, 0}; memcpy(&v.lo, &d, 8); return v; }
static inline V vx80(long double x) {
    unsigned char b[16];
    memcpy(b, &x, 16);
    V v;
    memcpy(&v.lo, b, 8);
    memcpy(&v.hi, b + 8, 2);
    return v;
}
static inline float32_t sf32(V v) { float32_t r = {(uint32_t)v.lo}; return r; }
static inline float64_t sf64(V v) { float64_t r = {v.lo}; return r; }
static inline extFloat80_t sx80(V v) { extFloat80_t r; r.signif = v.lo; r.signExp = v.hi; return r; }
static inline V wf32(float32_t f) { return (V){f.v, 0}; }
static inline V wf64(float64_t f) { return (V){f.v, 0}; }
static inline V wx80(extFloat80_t f) { return (V){f.signif, f.signExp}; }
static inline V vint(uint64_t u) { return (V){u, 0}; }

static int is_nan(int fmt, V v) {
    switch (fmt) {
    case F32: return ((uint32_t)v.lo & 0x7FFFFFFF) > 0x7F800000;
    case F64: return (v.lo & 0x7FFFFFFFFFFFFFFF) > 0x7FF0000000000000;
    case X80: return (v.hi & 0x7FFF) == 0x7FFF && (v.lo << 1) != 0;
    default: return 0;
    }
}

/* extF80 encodings that no arithmetic produces: a pseudo-denormal (exponent 0, integer bit set), which
 * x87 accepts as a value; and an unnormal, pseudo-infinity or pseudo-NaN (exponent not 0, integer bit
 * clear), which x87 rejects as an invalid operand. */
static int pseudo_denormal(V v) { return (v.hi & 0x7FFF) == 0 && v.lo >> 63; }
static int unsupported(V v) { return (v.hi & 0x7FFF) != 0 && !(v.lo >> 63); }
static int noncanonical(V v) { return pseudo_denormal(v) || unsupported(v); }

static int is_zero(int fmt, V v) {
    switch (fmt) {
    case F32: return ((uint32_t)v.lo & 0x7FFFFFFF) == 0;
    case F64: return (v.lo & 0x7FFFFFFFFFFFFFFF) == 0;
    default: return (v.hi & 0x7FFF) == 0 && v.lo == 0;
    }
}

static int is_inf(int fmt, V v) {
    switch (fmt) {
    case F32: return ((uint32_t)v.lo & 0x7FFFFFFF) == 0x7F800000;
    case F64: return (v.lo & 0x7FFFFFFFFFFFFFFF) == 0x7FF0000000000000;
    default: return (v.hi & 0x7FFF) == 0x7FFF && v.lo == 0x8000000000000000;
    }
}

static int is_inf_times_zero(int fmt, V a, V b) {
    return (is_inf(fmt, a) && is_zero(fmt, b)) || (is_zero(fmt, a) && is_inf(fmt, b));
}

static int same(int fmt, V a, V b) {
    switch (fmt) {
    case F32: case I32: case U32: return (uint32_t)a.lo == (uint32_t)b.lo;
    case F64: case I64: case U64: return a.lo == b.lo;
    case X80: return a.lo == b.lo && a.hi == b.hi;
    case BOOL: return (a.lo & 1) == (b.lo & 1);
    }
    return 0;
}

static void show(char *out, size_t n, int fmt, V v) {
    switch (fmt) {
    case F32: case I32: case U32: snprintf(out, n, "%08x", (unsigned)(uint32_t)v.lo); break;
    case X80: snprintf(out, n, "%04x:%016llx", v.hi, (unsigned long long)v.lo); break;
    case BOOL: snprintf(out, n, "%d", (int)(v.lo & 1)); break;
    default: snprintf(out, n, "%016llx", (unsigned long long)v.lo); break;
    }
}

/* ---- the operations: hardware (H) and SoftFloat (S) ------------------------------------------- */

typedef V (*fn)(V, V, V);

#define BIN(F, T, H, W, SF, SW, NAME, OP, SFN)                                                            \
    static NOINLINE V h_##F##_##NAME(V a, V b, V c) { (void)c; T x = H(a), y = H(b); return W(x OP y); } \
    static V s_##F##_##NAME(V a, V b, V c) { (void)c; return SW(SFN(SF(a), SF(b))); }
#define BINS(F, T, H, W, SF, SW, P)                     \
    BIN(F, T, H, W, SF, SW, add, +, P##_add)            \
    BIN(F, T, H, W, SF, SW, sub, -, P##_sub)            \
    BIN(F, T, H, W, SF, SW, mul, *, P##_mul)            \
    BIN(F, T, H, W, SF, SW, div, /, P##_div)
/* SSE: the instruction itself, with the first operand as src1 (`addss b, a` computes a = a + b). C does
 * not fix this order: clang commutes the operands of fadd and fmul, so which of two NaN operands C's
 * `a + b` returns depends on register allocation. */
#define SSE(F, T, H, W, SF, SW, NAME, INSN, SFN)                                                   \
    static NOINLINE V h_##F##_##NAME(V a, V b, V c) {                                             \
        (void)c;                                                                                  \
        T x = H(a), y = H(b);                                                                     \
        __asm__(INSN " %1, %0" : "+x"(x) : "x"(y));                                               \
        return W(x);                                                                              \
    }                                                                                             \
    static V s_##F##_##NAME(V a, V b, V c) { (void)c; return SW(SFN(SF(a), SF(b))); }
#define SSES(F, T, H, W, SF, SW, P, S)                     \
    SSE(F, T, H, W, SF, SW, add, "add" S, P##_add)         \
    SSE(F, T, H, W, SF, SW, sub, "sub" S, P##_sub)         \
    SSE(F, T, H, W, SF, SW, mul, "mul" S, P##_mul)         \
    SSE(F, T, H, W, SF, SW, div, "div" S, P##_div)
SSES(f32, float, hf32, vf32, sf32, wf32, f32, "ss")
SSES(f64, double, hf64, vf64, sf64, wf64, f64, "sd")
BINS(x80, long double, hx80, vx80, sx80, wx80, extF80)

#define CALL1(F, T, H, W, SF, SW, NAME, HFN, SFEXPR)                                                     \
    static NOINLINE V h_##F##_##NAME(V a, V b, V c) { (void)b; (void)c; T x = H(a); return W(HFN(x)); } \
    static V s_##F##_##NAME(V a, V b, V c) { (void)b; (void)c; return SW(SFEXPR); }
#define UNARY(F, T, H, W, SF, SW, P, SUF)                                                                 \
    CALL1(F, T, H, W, SF, SW, sqrt, __builtin_sqrt##SUF, P##_sqrt(SF(a)))                               \
    CALL1(F, T, H, W, SF, SW, trunc, __builtin_trunc##SUF, P##_roundToInt(SF(a), softfloat_round_minMag, 0)) \
    CALL1(F, T, H, W, SF, SW, floor, __builtin_floor##SUF, P##_roundToInt(SF(a), softfloat_round_min, 0))    \
    CALL1(F, T, H, W, SF, SW, ceil, __builtin_ceil##SUF, P##_roundToInt(SF(a), softfloat_round_max, 0))      \
    CALL1(F, T, H, W, SF, SW, round, __builtin_round##SUF,                                               \
          P##_roundToInt(SF(a), softfloat_round_near_maxMag, 0))                                         \
    CALL1(F, T, H, W, SF, SW, roundeven, __builtin_roundeven##SUF,                                       \
          P##_roundToInt(SF(a), softfloat_round_near_even, 0))                                           \
    CALL1(F, T, H, W, SF, SW, rint, __builtin_rint##SUF, P##_roundToInt(SF(a), softfloat_round_near_even, 1))
UNARY(f32, float, hf32, vf32, sf32, wf32, f32, f)
UNARY(f64, double, hf64, vf64, sf64, wf64, f64, )
UNARY(x80, long double, hx80, vx80, sx80, wx80, extF80, l)

/* Sign operations: SoftFloat has none; the composition is a bit operation. */
static NOINLINE V h_f32_neg(V a, V b, V c) { (void)b; (void)c; return vf32(-hf32(a)); }
static NOINLINE V h_f64_neg(V a, V b, V c) { (void)b; (void)c; return vf64(-hf64(a)); }
static NOINLINE V h_x80_neg(V a, V b, V c) { (void)b; (void)c; return vx80(-hx80(a)); }
static V s_f32_neg(V a, V b, V c) { (void)b; (void)c; return (V){a.lo ^ 0x80000000u, 0}; }
static V s_f64_neg(V a, V b, V c) { (void)b; (void)c; return (V){a.lo ^ 0x8000000000000000u, 0}; }
static V s_x80_neg(V a, V b, V c) { (void)b; (void)c; return (V){a.lo, (uint16_t)(a.hi ^ 0x8000)}; }
static NOINLINE V h_f32_fabs(V a, V b, V c) { (void)b; (void)c; return vf32(__builtin_fabsf(hf32(a))); }
static NOINLINE V h_f64_fabs(V a, V b, V c) { (void)b; (void)c; return vf64(__builtin_fabs(hf64(a))); }
static NOINLINE V h_x80_fabs(V a, V b, V c) { (void)b; (void)c; return vx80(__builtin_fabsl(hx80(a))); }
static V s_f32_fabs(V a, V b, V c) { (void)b; (void)c; return (V){a.lo & 0x7FFFFFFFu, 0}; }
static V s_f64_fabs(V a, V b, V c) { (void)b; (void)c; return (V){a.lo & 0x7FFFFFFFFFFFFFFFu, 0}; }
static V s_x80_fabs(V a, V b, V c) { (void)b; (void)c; return (V){a.lo, (uint16_t)(a.hi & 0x7FFF)}; }

/* IEEE remainder against glibc's remainder (x87 fprem1 for long double). */
static NOINLINE V h_f32_rem(V a, V b, V c) { (void)c; return vf32(remainderf(hf32(a), hf32(b))); }
static NOINLINE V h_f64_rem(V a, V b, V c) { (void)c; return vf64(remainder(hf64(a), hf64(b))); }
static NOINLINE V h_x80_rem(V a, V b, V c) { (void)c; return vx80(remainderl(hx80(a), hx80(b))); }
static V s_f32_rem(V a, V b, V c) { (void)c; return wf32(f32_rem(sf32(a), sf32(b))); }
static V s_f64_rem(V a, V b, V c) { (void)c; return wf64(f64_rem(sf64(a), sf64(b))); }
static V s_x80_rem(V a, V b, V c) { (void)c; return wx80(extF80_rem(sx80(a), sx80(b))); }

/* Fused multiply-add: the instruction vfmadd132s[sd] computing a = a * b + c. x86 FMA returns the first
 * NaN in the order of the formula (first multiplicand, second, addend), and which multiplicand comes
 * first depends on the encoding the compiler picks (132, 213 or 231): glibc's fma(x, y, z) is
 * vfmadd213sd, which puts y first. Without FMA hardware the reference is glibc's fma. SoftFloat 3e has no
 * extF80 mulAdd. */
static int have_fma;
static NOINLINE __attribute__((target("fma"))) V h_f32_fma(V a, V b, V c) {
    float x = hf32(a), y = hf32(b), z = hf32(c);
    if (!have_fma) return vf32(fmaf(x, y, z));
    __asm__("vfmadd132ss %2, %1, %0" : "+x"(x) : "x"(z), "x"(y));
    return vf32(x);
}
static NOINLINE __attribute__((target("fma"))) V h_f64_fma(V a, V b, V c) {
    double x = hf64(a), y = hf64(b), z = hf64(c);
    if (!have_fma) return vf64(fma(x, y, z));
    __asm__("vfmadd132sd %2, %1, %0" : "+x"(x) : "x"(z), "x"(y));
    return vf64(x);
}
static V s_f32_fma(V a, V b, V c) { return wf32(f32_mulAdd(sf32(a), sf32(b), sf32(c))); }
static V s_f64_fma(V a, V b, V c) { return wf64(f64_mulAdd(sf64(a), sf64(b), sf64(c))); }

/* Comparisons: every fcmp predicate as C writes it, against its composition from eq/lt/le. */
#define UNO(P, SF, a, b) (!P##_eq(SF(a), SF(a)) || !P##_eq(SF(b), SF(b)))
#define CMP(F, T, H, SF, P, NAME, HEXPR, SEXPR)                                                    \
    static NOINLINE V h_##F##_##NAME(V a, V b, V c) { (void)c; T x = H(a), y = H(b); return vint(HEXPR); } \
    static V s_##F##_##NAME(V a, V b, V c) { (void)c; return vint(SEXPR); }
#define CMPS(F, T, H, SF, P)                                                                       \
    CMP(F, T, H, SF, P, oeq, x == y, P##_eq(SF(a), SF(b)))                                       \
    CMP(F, T, H, SF, P, olt, x < y, P##_lt(SF(a), SF(b)))                                        \
    CMP(F, T, H, SF, P, ole, x <= y, P##_le(SF(a), SF(b)))                                       \
    CMP(F, T, H, SF, P, ogt, x > y, P##_lt(SF(b), SF(a)))                                        \
    CMP(F, T, H, SF, P, oge, x >= y, P##_le(SF(b), SF(a)))                                       \
    CMP(F, T, H, SF, P, one, x < y || x > y, P##_lt(SF(a), SF(b)) || P##_lt(SF(b), SF(a)))       \
    CMP(F, T, H, SF, P, ord, !__builtin_isunordered(x, y), !UNO(P, SF, a, b))                    \
    CMP(F, T, H, SF, P, ueq, __builtin_isunordered(x, y) || x == y, UNO(P, SF, a, b) || P##_eq(SF(a), SF(b))) \
    CMP(F, T, H, SF, P, ugt, !(x <= y), !P##_le(SF(a), SF(b)))                                   \
    CMP(F, T, H, SF, P, uge, !(x < y), !P##_lt(SF(a), SF(b)))                                    \
    CMP(F, T, H, SF, P, ult, !(x >= y), !P##_le(SF(b), SF(a)))                                   \
    CMP(F, T, H, SF, P, ule, !(x > y), !P##_lt(SF(b), SF(a)))                                    \
    CMP(F, T, H, SF, P, une, x != y, !P##_eq(SF(a), SF(b)))                                      \
    CMP(F, T, H, SF, P, uno, __builtin_isunordered(x, y), UNO(P, SF, a, b))                      \
    CMP(F, T, H, SF, P, eq_signaling, x == y, P##_eq_signaling(SF(a), SF(b)))                    \
    CMP(F, T, H, SF, P, lt_quiet, x < y, P##_lt_quiet(SF(a), SF(b)))                             \
    CMP(F, T, H, SF, P, le_quiet, x <= y, P##_le_quiet(SF(a), SF(b)))
CMPS(f32, float, hf32, sf32, f32)
CMPS(f64, double, hf64, sf64, f64)
CMPS(x80, long double, hx80, sx80, extF80)

/* Conversions between formats. */
#define CONV(NAME, G, H, W, SEXPR)                                                                \
    static NOINLINE V h_##NAME(V a, V b, V c) { (void)b; (void)c; return W(G(H(a))); }       \
    static V s_##NAME(V a, V b, V c) { (void)b; (void)c; return SEXPR; }
static inline double d_of_f(float f) { return f; }
static inline long double x_of_f(float f) { return f; }
static inline float f_of_d(double d) { return (float)d; }
static inline long double x_of_d(double d) { return d; }
static inline float f_of_x(long double x) { return (float)x; }
static inline double d_of_x(long double x) { return (double)x; }
CONV(f32_f64, d_of_f, hf32, vf64, wf64(f32_to_f64(sf32(a))))
CONV(f32_x80, x_of_f, hf32, vx80, wx80(f32_to_extF80(sf32(a))))
CONV(f64_f32, f_of_d, hf64, vf32, wf32(f64_to_f32(sf64(a))))
CONV(f64_x80, x_of_d, hf64, vx80, wx80(f64_to_extF80(sf64(a))))
CONV(x80_f32, f_of_x, hx80, vf32, wf32(extF80_to_f32(sx80(a))))
CONV(x80_f64, d_of_x, hx80, vf64, wf64(extF80_to_f64(sx80(a))))
#undef CONV

/* To integers, truncating (C's conversion): roundingMode = minMag, exact = false. */
#define TOINT(F, T, H, SF, P, I, CT)                                                                    \
    static NOINLINE V h_##F##_##I(V a, V b, V c) { (void)b; (void)c; T x = H(a); return vint((uint64_t)(CT)x); } \
    static V s_##F##_##I(V a, V b, V c) { (void)b; (void)c; return vint((uint64_t)(CT)P##_to_##I##_r_minMag(SF(a), 0)); }
#define TOINTS(F, T, H, SF, P)                  \
    TOINT(F, T, H, SF, P, i32, int32_t)         \
    TOINT(F, T, H, SF, P, i64, int64_t)         \
    TOINT(F, T, H, SF, P, ui32, uint32_t)       \
    TOINT(F, T, H, SF, P, ui64, uint64_t)
TOINTS(f32, float, hf32, sf32, f32)
TOINTS(f64, double, hf64, sf64, f64)
TOINTS(x80, long double, hx80, sx80, extF80)

/* lrint and lround (long is 64 bits): the current rounding mode (nearest even), and nearest away. */
#define LROUND(F, T, H, SF, P, SUF)                                                                        \
    static NOINLINE V h_##F##_lrint(V a, V b, V c) { (void)b; (void)c; return vint((uint64_t)__builtin_lrint##SUF(H(a))); } \
    static V s_##F##_lrint(V a, V b, V c) { (void)b; (void)c; return vint((uint64_t)P##_to_i64(SF(a), softfloat_round_near_even, 1)); } \
    static NOINLINE V h_##F##_lround(V a, V b, V c) { (void)b; (void)c; return vint((uint64_t)__builtin_lround##SUF(H(a))); } \
    static V s_##F##_lround(V a, V b, V c) { (void)b; (void)c; return vint((uint64_t)P##_to_i64(SF(a), softfloat_round_near_maxMag, 0)); }
LROUND(f32, float, hf32, sf32, f32, f)
LROUND(f64, double, hf64, sf64, f64, )
LROUND(x80, long double, hx80, sx80, extF80, l)

/* From integers. */
#define FROMINT(I, CT, F, T, W, SW, P)                                                                 \
    static NOINLINE V h_##I##_##F(V a, V b, V c) { (void)b; (void)c; CT n = (CT)a.lo; return W((T)n); } \
    static V s_##I##_##F(V a, V b, V c) { (void)b; (void)c; return SW(I##_to_##P((CT)a.lo)); }
#define FROMINTS(I, CT)                                         \
    FROMINT(I, CT, f32, float, vf32, wf32, f32)                 \
    FROMINT(I, CT, f64, double, vf64, wf64, f64)                \
    FROMINT(I, CT, x80, long double, vx80, wx80, extF80)
FROMINTS(i32, int32_t)
FROMINTS(i64, int64_t)
FROMINTS(ui32, uint32_t)
FROMINTS(ui64, uint64_t)

typedef struct {
    const char *name;
    int arity, in, out;
    fn h, s;
} Op;

#define OP(F, NAME, AR, IN, OUT) {#F "_" #NAME, AR, IN, OUT, h_##F##_##NAME, s_##F##_##NAME}
#define FLOAT_OPS(F, FM)                                                                              \
    OP(F, add, 2, FM, FM), OP(F, sub, 2, FM, FM), OP(F, mul, 2, FM, FM), OP(F, div, 2, FM, FM),       \
    OP(F, rem, 2, FM, FM), OP(F, sqrt, 1, FM, FM), OP(F, neg, 1, FM, FM), OP(F, fabs, 1, FM, FM),     \
    OP(F, trunc, 1, FM, FM), OP(F, floor, 1, FM, FM), OP(F, ceil, 1, FM, FM), OP(F, round, 1, FM, FM), \
    OP(F, roundeven, 1, FM, FM), OP(F, rint, 1, FM, FM),                                            \
    OP(F, oeq, 2, FM, BOOL), OP(F, olt, 2, FM, BOOL), OP(F, ole, 2, FM, BOOL), OP(F, ogt, 2, FM, BOOL), \
    OP(F, oge, 2, FM, BOOL), OP(F, one, 2, FM, BOOL), OP(F, ord, 2, FM, BOOL), OP(F, ueq, 2, FM, BOOL), \
    OP(F, ugt, 2, FM, BOOL), OP(F, uge, 2, FM, BOOL), OP(F, ult, 2, FM, BOOL), OP(F, ule, 2, FM, BOOL), \
    OP(F, une, 2, FM, BOOL), OP(F, uno, 2, FM, BOOL), OP(F, eq_signaling, 2, FM, BOOL),               \
    OP(F, lt_quiet, 2, FM, BOOL), OP(F, le_quiet, 2, FM, BOOL),                                     \
    OP(F, i32, 1, FM, I32), OP(F, i64, 1, FM, I64), OP(F, ui32, 1, FM, U32), OP(F, ui64, 1, FM, U64), \
    OP(F, lrint, 1, FM, I64), OP(F, lround, 1, FM, I64)
#define CONV_OP(A, B, IN, OUT) {#A "_" #B, 1, IN, OUT, h_##A##_##B, s_##A##_##B}

static const Op ops[] = {
    FLOAT_OPS(f32, F32), OP(f32, fma, 3, F32, F32),
    FLOAT_OPS(f64, F64), OP(f64, fma, 3, F64, F64),
    FLOAT_OPS(x80, X80),
    CONV_OP(f32, f64, F32, F64), CONV_OP(f32, x80, F32, X80), CONV_OP(f64, f32, F64, F32),
    CONV_OP(f64, x80, F64, X80), CONV_OP(x80, f32, X80, F32), CONV_OP(x80, f64, X80, F64),
    CONV_OP(i32, f32, I32, F32), CONV_OP(i32, f64, I32, F64), CONV_OP(i32, x80, I32, X80),
    CONV_OP(i64, f32, I64, F32), CONV_OP(i64, f64, I64, F64), CONV_OP(i64, x80, I64, X80),
    CONV_OP(ui32, f32, U32, F32), CONV_OP(ui32, f64, U32, F64), CONV_OP(ui32, x80, U32, X80),
    CONV_OP(ui64, f32, U64, F32), CONV_OP(ui64, f64, U64, F64), CONV_OP(ui64, x80, U64, X80),
};
#define NOPS ((int)(sizeof ops / sizeof ops[0]))

/* ---- directed operands ----------------------------------------------------------------------- */

static const double directed_doubles[] = {
    0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 0.49999999999999994, 0.1, 1.0 / 3.0, 1e30, 1e-30,
    8388607.5, 8388608.0, 16777216.0, 4503599627370495.5, 4503599627370496.0, 9007199254740992.0,
    2147483647.0, 2147483647.5, 2147483648.0, 2147483649.0, 4294967295.0, 4294967296.0,
    9223372036854774784.0, 9223372036854775808.0, 18446744073709549568.0, 18446744073709551616.0,
};

static V directed[3][160];
static int ndirected[3];

static void add_directed(int f, V v) { directed[f][ndirected[f]++] = v; }

static void build_directed(void) {
    static const uint32_t f32_bits[] = {
        0x00000001, 0x00000003, 0x00400000, 0x007FFFFF, 0x00800000, 0x00C00000, 0x3F800001, 0x3F7FFFFF,
        0x33800000, 0x33000000, 0x7F000000, 0x7F7FFFFF, 0x7F800000,
        0x7FC00000, 0x7FC00001, 0x7FD55555, 0x7FFFFFFF, /* quiet NaNs */
        0x7F800001, 0x7FA00000, 0x7FBFFFFF, 0x7F812345, /* signalling NaNs */
    };
    static const uint64_t f64_bits[] = {
        0x0000000000000001, 0x0000000000000003, 0x0008000000000000, 0x000FFFFFFFFFFFFF,
        0x0010000000000000, 0x0018000000000000, 0x3FF0000000000001, 0x3FEFFFFFFFFFFFFF,
        0x3CA0000000000000, 0x3C90000000000000, 0x7FE0000000000000, 0x7FEFFFFFFFFFFFFF,
        0x7FF0000000000000,
        0x7FF8000000000000, 0x7FF8000000000001, 0x7FFD555555555555, 0x7FFFFFFFFFFFFFFF,
        0x7FF0000000000001, 0x7FF4000000000000, 0x7FF7FFFFFFFFFFFF, 0x7FF0000000012345,
    };
    static const struct { uint16_t e; uint64_t s; } x80_bits[] = {
        {0, 1}, {0, 3}, {0, 0x4000000000000000}, {0, 0x7FFFFFFFFFFFFFFF}, {1, 0x8000000000000000},
        {1, 0xC000000000000000}, {0x3FFF, 0x8000000000000001}, {0x3FFE, 0xFFFFFFFFFFFFFFFF},
        {0x3FBF, 0x8000000000000000}, {0x3FBE, 0x8000000000000000}, {0x7FFE, 0x8000000000000000},
        {0x7FFE, 0xFFFFFFFFFFFFFFFF}, {0x7FFF, 0x8000000000000000},
        {0x7FFF, 0xC000000000000000}, {0x7FFF, 0xC000000000000001}, {0x7FFF, 0xD555555555555555},
        {0x7FFF, 0xFFFFFFFFFFFFFFFF}, /* quiet NaNs */
        {0x7FFF, 0x8000000000000001}, {0x7FFF, 0xA000000000000000}, {0x7FFF, 0xBFFFFFFFFFFFFFFF},
        {0x7FFF, 0x8000000000012345}, /* signalling NaNs */
        {0, 0x8000000000000000}, {0, 0x8000000000000001}, {0, 0xFFFFFFFFFFFFFFFF}, /* pseudo-denormals */
        {1, 0x4000000000000000}, {0x3FFF, 0x4000000000000000}, {0x3FFF, 0}, {0x7FFE, 0x7FFFFFFFFFFFFFFF},
        {0x4040, 0x0000000000000001}, /* unnormals */
        {0x7FFF, 0}, /* pseudo-infinity */
        {0x7FFF, 0x4000000000000000}, {0x7FFF, 0x0000000000000001}, {0x7FFF, 0x7FFFFFFFFFFFFFFF}, /* pseudo-NaNs */
    };
    for (int sign = 0; sign < 2; sign++) {
        for (size_t i = 0; i < sizeof directed_doubles / sizeof directed_doubles[0]; i++) {
            double d = sign ? -directed_doubles[i] : directed_doubles[i];
            add_directed(F32, vf32((float)d));
            add_directed(F64, vf64(d));
            add_directed(X80, vx80((long double)d));
        }
        for (size_t i = 0; i < sizeof f32_bits / sizeof f32_bits[0]; i++)
            add_directed(F32, (V){f32_bits[i] | (uint32_t)sign << 31, 0});
        for (size_t i = 0; i < sizeof f64_bits / sizeof f64_bits[0]; i++)
            add_directed(F64, (V){f64_bits[i] | (uint64_t)sign << 63, 0});
        for (size_t i = 0; i < sizeof x80_bits / sizeof x80_bits[0]; i++)
            add_directed(X80, (V){x80_bits[i].s, (uint16_t)(x80_bits[i].e | sign << 15)});
    }
    /* x80 values that are not doubles: 1 + half an ulp of double (exact in extF80), 2^63 - 1. */
    add_directed(X80, (V){0x8000000000000400, 0x3FFF});
    add_directed(X80, (V){0xFFFFFFFFFFFFFFFE, 0x403D});
    add_directed(X80, (V){0xFFFFFFFFFFFFFFFF, 0x403E});
    add_directed(X80, (V){0x8000000000000001, 0xC03E});
}

static const uint64_t directed_ints[] = {
    0, 1, 2, 3, 0x7F, 0x80, 0xFF, 0x7FFF, 0x8000, 0xFFFF, 0xFFFFFF, 0x1000001, 0x1000003, 0x7FFFFFFF,
    0x80000000, 0x80000001, 0xFFFFFFFF, 0x100000000, 0x1FFFFFFFFFFFFF, 0x20000000000001,
    0x20000000000003, 0x7FFFFFFFFFFFFFFF, 0x8000000000000000, 0x8000000000000001, 0xFFFFFFFFFFFFFFFF,
    0xFFFFFFFFFFFFFFFE, 0xFFFFFFFF80000000, 0xFFFFFFFF7FFFFFFF, 0xFFFFFF7FFFFFFFFF, 0x0000008000000001,
};
#define NDIRECTED_INTS ((int)(sizeof directed_ints / sizeof directed_ints[0]))

/* ---- random operands ------------------------------------------------------------------------- */

static inline uint64_t next(uint64_t *s) { /* splitmix64 */
    uint64_t z = (*s += 0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EB;
    return z ^ (z >> 31);
}

/* Field widths: significand bits stored, exponent bits, bias. */
static const int sig_bits[3] = {23, 52, 64}, exp_bits[3] = {8, 11, 15};

static V pack(int f, unsigned sign, unsigned exp, uint64_t sig) {
    switch (f) {
    case F32: return (V){(uint64_t)sign << 31 | (uint64_t)exp << 23 | (sig & 0x7FFFFF), 0};
    case F64: return (V){(uint64_t)sign << 63 | (uint64_t)exp << 52 | (sig & 0xFFFFFFFFFFFFF), 0};
    default: /* canonical: integer bit set iff the exponent is not zero */
        return (V){exp ? sig | 0x8000000000000000 : sig & 0x7FFFFFFFFFFFFFFF, (uint16_t)(sign << 15 | exp)};
    }
}
static unsigned exp_of(int f, V v) {
    switch (f) {
    case F32: return (unsigned)(v.lo >> 23) & 0xFF;
    case F64: return (unsigned)(v.lo >> 52) & 0x7FF;
    default: return v.hi & 0x7FFF;
    }
}
static int bias(int f) { return (1 << (exp_bits[f] - 1)) - 1; }

/* An integer-valued float with up to `bits` significant bits, perhaps plus a half. */
static V random_integer(int f, uint64_t *s) {
    uint64_t r = next(s);
    int bits = (int)(r % 66);
    uint64_t m = bits >= 64 ? next(s) : next(s) & ((UINT64_C(1) << bits) - 1);
    long double x = (long double)m;
    if (r >> 60 & 1) x += 0.5L;
    if (r >> 61 & 1) x = -x;
    if (f == F32) return vf32((float)x);
    if (f == F64) return vf64((double)x);
    return vx80(x);
}

/* One operand; `near`, if given, is an operand whose exponent this one should be close to. */
static V random_float(int f, uint64_t *s, const V *near, int near_exp_offset) {
    uint64_t r = next(s), sig = next(s);
    unsigned emax = (1u << exp_bits[f]) - 1, sign = (unsigned)(r & 1);
    int mode = (int)((r >> 8) % 16);
    if (near && mode < 6) { /* near-equal exponent, or a few ulps away */
        if (mode < 2) {
            uint64_t d = (r >> 20) % 5;
            V v = *near;
            if (f == X80) { v.lo += d - 2; v.hi ^= (uint16_t)((r >> 30 & 1) << 15); return noncanonical(v) ? *near : v; }
            v.lo += d - 2;
            if (r >> 30 & 1) v.lo ^= f == F32 ? 0x80000000u : 0x8000000000000000u;
            if (f == F32) v.lo &= 0xFFFFFFFF;
            return v;
        }
        int e = (int)exp_of(f, *near) + near_exp_offset + (int)((r >> 20) % 5) - 2;
        if (e < 0) e = 0;
        if (e > (int)emax) e = (int)emax;
        return pack(f, sign, (unsigned)e, sig);
    }
    switch (mode % 8) {
    case 0: case 1: /* uniform bits (for extF80: a canonical encoding) */
        if (f == F32) return (V){sig & 0xFFFFFFFF, 0};
        if (f == F64) return (V){sig, 0};
        return pack(f, sign, (unsigned)(r >> 16) & 0x7FFF, sig);
    case 2: /* tiny exponents: subnormal and the first few normal binades */
        return pack(f, sign, (unsigned)((r >> 16) % (unsigned)(sig_bits[f] + 3)), sig);
    case 3: /* huge exponents, near overflow */
        return pack(f, sign, emax - 1 - (unsigned)((r >> 16) % 4), sig);
    case 4: /* exponents near 1 */
        return pack(f, sign, (unsigned)(bias(f) - 3 + (int)((r >> 16) % 7)), sig);
    case 5: case 6:
        return random_integer(f, s);
    default: /* a directed value, or for extF80 sometimes a non-canonical encoding */
        if (f == X80 && (r >> 16 & 3) == 0) return (V){sig, (uint16_t)((r >> 20) & 1 ? 0 : (r >> 24) & 0xFFFF)};
        return directed[f][(r >> 16) % (uint64_t)ndirected[f]];
    }
}

static V random_int(uint64_t *s) {
    uint64_t r = next(s), x = next(s);
    switch (r % 4) {
    case 0: return vint(x);
    case 1: return vint(x >> (r >> 8) % 64);
    case 2: return vint((uint64_t)-(int64_t)(x >> (r >> 8) % 64));
    default: return vint(directed_ints[(r >> 8) % NDIRECTED_INTS] + ((r >> 16) % 5) - 2);
    }
}

/* ---- classification and bookkeeping --------------------------------------------------------- */

enum {
    VALUE,          /* results differ, not both NaN */
    ZERO_SIGN,      /* +0 against -0 */
    NAN_BITS,       /* both NaN, different bits */
    FMA_INVALID,    /* fma(0, inf, NaN) or fma(inf, 0, NaN): x86 returns the quieted NaN addend, SoftFloat
                       the default NaN */
    PD_VALUE,       /* an extF80 pseudo-denormal operand */
    PD_NAN_BITS,
    UNSUP_VALUE,    /* an extF80 unnormal, pseudo-infinity or pseudo-NaN operand */
    UNSUP_NAN_BITS,
    OUT_OF_RANGE,   /* to-integer conversion undefined in C */
    NCLASSES
};
static const char *const class_names[] = {"value", "zero-sign", "nan-bits", "fma-invalid-nan",
                                          "pseudo-denormal-value", "pseudo-denormal-nan-bits",
                                          "unsupported-value", "unsupported-nan-bits", "out-of-range"};

/* The differences that are understood and accepted, by operation-name pattern (a prefix, or "*" for
 * any) and class. Filled in from the measurement; see softfloat.rs for what each one is. */
static const struct { const char *prefix; int cls; } expected[] = {
    {"f32_fma", FMA_INVALID},
    {"f64_fma", FMA_INVALID},
    {"f64_rem", NAN_BITS},  /* glibc remainder returns (x * y) / (x * y), whose NaN is y's */
    {"f64_rem", ZERO_SIGN}, /* glibc remainder gives -0 for some x > 0 and subnormal y */
    {"x80_", PD_VALUE},
    {"x80_", PD_NAN_BITS},
    {"x80_", UNSUP_VALUE},
    {"x80_", UNSUP_NAN_BITS},
    {"*", OUT_OF_RANGE},
};

static int is_expected(const char *name, int cls) {
    for (size_t i = 0; i < sizeof expected / sizeof expected[0]; i++) {
        const char *p = expected[i].prefix;
        if (expected[i].cls == cls && (!strcmp(p, "*") || !strncmp(name, p, strlen(p)))) return 1;
    }
    return 0;
}

typedef struct {
    atomic_ulong compared[3]; /* directed, random, exhaustive */
    atomic_ulong count[3][NCLASSES];
    pthread_mutex_t lock;
    int have_example[NCLASSES];
    char example[NCLASSES][200];
} Stats;
static Stats stats[NOPS];

/* Whether the C conversion of `a` (format `in`) to integer type `out` is defined. */
static int int_in_range(const char *name, int in, int out, V a) {
    long double x;
    if (in == F32) x = hf32(a);
    else if (in == F64) x = hf64(a);
    else { if (noncanonical(a)) return 1; x = hx80(a); }
    if (x != x) return 0;
    long double t = strstr(name, "lrint") ? __builtin_rintl(x) : strstr(name, "lround") ? __builtin_roundl(x)
                                                                                    : __builtin_truncl(x);
    switch (out) {
    case I32: return t >= -2147483648.0L && t <= 2147483647.0L;
    case U32: return t >= 0 && t <= 4294967295.0L;
    case I64: return t >= -9223372036854775808.0L && t < 9223372036854775808.0L;
    case U64: return t >= 0 && t < 18446744073709551616.0L;
    }
    return 1;
}

/* Counts kept by one thread, added to `stats` at the end. */
typedef struct {
    unsigned long compared, count[NCLASSES];
} Local;

static void merge(int k, int phase, const Local *l) {
    atomic_fetch_add(&stats[k].compared[phase], l->compared);
    for (int c = 0; c < NCLASSES; c++) atomic_fetch_add(&stats[k].count[phase][c], l->count[c]);
}

static void check(int k, Local *l, V a, V b, V c) {
    const Op *op = &ops[k];
    Stats *st = &stats[k];
    V h = op->h(a, b, c), s = op->s(a, b, c);
    l->compared++;
    if (same(op->out, h, s)) return;
    int cls;
    if (op->out >= I32 && op->in <= X80 && !int_in_range(op->name, op->in, op->out, a)) cls = OUT_OF_RANGE;
    else {
        int x80 = op->in == X80, two = op->arity > 1;
        int unsup = x80 && (unsupported(a) || (two && unsupported(b)));
        int pd = x80 && (pseudo_denormal(a) || (two && pseudo_denormal(b)));
        int nan = is_nan(op->out, h) && is_nan(op->out, s);
        int zeros = op->out <= X80 && is_zero(op->out, h) && is_zero(op->out, s);
        int fma_invalid = op->arity == 3 && nan && is_nan(op->in, c) && is_inf_times_zero(op->in, a, b);
        cls = unsup ? (nan ? UNSUP_NAN_BITS : UNSUP_VALUE)
            : pd    ? (nan ? PD_NAN_BITS : PD_VALUE)
            : fma_invalid ? FMA_INVALID
            : nan   ? NAN_BITS
            : zeros ? ZERO_SIGN
                    : VALUE;
    }
    l->count[cls]++;
    if (st->have_example[cls]) return;
    pthread_mutex_lock(&st->lock);
    if (!st->have_example[cls]) {
        char x[3][32], hs[32], ss[32];
        V in[3] = {a, b, c};
        for (int i = 0; i < 3; i++) show(x[i], sizeof x[i], op->in, in[i]);
        show(hs, sizeof hs, op->out, h);
        show(ss, sizeof ss, op->out, s);
        snprintf(st->example[cls], sizeof st->example[cls], "(%s%s%s%s%s) hw %s sf %s", x[0],
                 op->arity > 1 ? ", " : "", op->arity > 1 ? x[1] : "", op->arity > 2 ? ", " : "",
                 op->arity > 2 ? x[2] : "", hs, ss);
        st->have_example[cls] = 1;
    }
    pthread_mutex_unlock(&st->lock);
}

/* ---- the three phases ---------------------------------------------------------------------- */

static void run_directed(int k) {
    const Op *op = &ops[k];
    Local l = {0};
    if (op->in >= I32) {
        for (int i = 0; i < NDIRECTED_INTS; i++)
            for (int d = -1; d <= 1; d++) {
                check(k, &l, vint(directed_ints[i] + (uint64_t)d), vint(0), vint(0));
                check(k, &l, vint((uint64_t)-(int64_t)(directed_ints[i] + (uint64_t)d)), vint(0), vint(0));
            }
        merge(k, 0, &l);
        return;
    }
    int f = op->in, n = ndirected[f];
    V z = {0, 0};
    if (op->arity == 1)
        for (int i = 0; i < n; i++) check(k, &l, directed[f][i], z, z);
    else if (op->arity == 2)
        for (int i = 0; i < n; i++)
            for (int j = 0; j < n; j++) check(k, &l, directed[f][i], directed[f][j], z);
    else
        for (int i = 0; i < n; i++)
            for (int j = 0; j < n; j++)
                for (int m = 0; m < n; m++) check(k, &l, directed[f][i], directed[f][j], directed[f][m]);
    merge(k, 0, &l);
}

static unsigned long per_op = 20000, nthreads = 1;
static int exhaustive = 0;

typedef struct { int k; unsigned long t; } Job;

static void *random_worker(void *p) {
    const Job *job = p;
    const Op *op = &ops[job->k];
    uint64_t s = 0x5EED0000 + (uint64_t)job->k * 0x10001 + job->t * 0x9E3779B9;
    unsigned long n = per_op / nthreads + (job->t < per_op % nthreads);
    Local l = {0};
    for (unsigned long i = 0; i < n; i++) {
        V a, b = {0, 0}, c = {0, 0};
        if (op->in >= I32) a = random_int(&s);
        else {
            int f = op->in;
            a = random_float(f, &s, NULL, 0);
            if (op->arity > 1) b = random_float(f, &s, &a, 0);
            if (op->arity > 2) {
                /* c near the product's exponent, for cancellation */
                int pe = (int)exp_of(f, a) + (int)exp_of(f, b) - bias(f);
                V p = pack(f, 0, (unsigned)(pe < 0 ? 0 : pe > (1 << exp_bits[f]) - 2 ? (1 << exp_bits[f]) - 2 : pe), 0);
                c = random_float(f, &s, &p, 0);
            }
        }
        check(job->k, &l, a, b, c);
    }
    merge(job->k, 1, &l);
    return NULL;
}

static void *exhaustive_worker(void *p) {
    const Job *job = p;
    uint64_t lo = (UINT64_C(1) << 32) * job->t / nthreads, hi = (UINT64_C(1) << 32) * (job->t + 1) / nthreads;
    V z = {0, 0};
    Local l = {0};
    for (uint64_t x = lo; x < hi; x++) {
        V a = vint(x);
        if (ops[job->k].in == I32) a = vint((uint64_t)(int64_t)(int32_t)(uint32_t)x);
        check(job->k, &l, a, z, z);
    }
    merge(job->k, 2, &l);
    return NULL;
}

static void parallel(int k, void *(*worker)(void *)) {
    pthread_t th[256];
    Job jobs[256];
    for (unsigned long t = 0; t < nthreads; t++) {
        jobs[t] = (Job){k, t};
        if (nthreads == 1) worker(&jobs[t]);
        else pthread_create(&th[t], NULL, worker, &jobs[t]);
    }
    if (nthreads > 1)
        for (unsigned long t = 0; t < nthreads; t++) pthread_join(th[t], NULL);
}

/* What the out-of-range to-integer conversions give, for the record. */
static void out_of_range_notes(void) {
    static const double xs[] = {NAN, -NAN, INFINITY, -INFINITY, 1e30, -1e30, 2147483648.0, -2147483649.0,
                                4294967296.0, -1.0, -2.0, 9223372036854775808.0, -9223372036854777856.0,
                                18446744073709551616.0};
    printf("out-of-range to-integer (hw / sf), value: f64->i32 f64->u32 f64->i64 f64->u64 | f80->i32 f80->u32 f80->i64 f80->u64\n");
    for (size_t i = 0; i < sizeof xs / sizeof xs[0]; i++) {
        V d = vf64(xs[i]), x = vx80((long double)xs[i]), z = {0, 0};
        printf("  %-22.17g", xs[i]);
        printf(" %x/%x", (unsigned)h_f64_i32(d, z, z).lo, (unsigned)s_f64_i32(d, z, z).lo);
        printf(" %x/%x", (unsigned)h_f64_ui32(d, z, z).lo, (unsigned)s_f64_ui32(d, z, z).lo);
        printf(" %llx/%llx", (unsigned long long)h_f64_i64(d, z, z).lo, (unsigned long long)s_f64_i64(d, z, z).lo);
        printf(" %llx/%llx |", (unsigned long long)h_f64_ui64(d, z, z).lo, (unsigned long long)s_f64_ui64(d, z, z).lo);
        printf(" %x/%x", (unsigned)h_x80_i32(x, z, z).lo, (unsigned)s_x80_i32(x, z, z).lo);
        printf(" %x/%x", (unsigned)h_x80_ui32(x, z, z).lo, (unsigned)s_x80_ui32(x, z, z).lo);
        printf(" %llx/%llx", (unsigned long long)h_x80_i64(x, z, z).lo, (unsigned long long)s_x80_i64(x, z, z).lo);
        printf(" %llx/%llx\n", (unsigned long long)h_x80_ui64(x, z, z).lo, (unsigned long long)s_x80_ui64(x, z, z).lo);
    }
}

int main(int argc, char **argv) {
    if (argc > 1) per_op = strtoul(argv[1], NULL, 0);
    if (argc > 2) nthreads = strtoul(argv[2], NULL, 0);
    if (argc > 3) exhaustive = atoi(argv[3]);
    const char *filter = argc > 4 ? argv[4] : "";
    if (nthreads < 1) nthreads = 1;
    if (nthreads > 256) nthreads = 256;
    have_fma = __builtin_cpu_supports("fma");
    printf("fma reference: %s\n", have_fma ? "vfmadd132 (a * b + c)" : "glibc fma (no FMA hardware)");
    build_directed();
    for (int k = 0; k < NOPS; k++) {
        pthread_mutex_init(&stats[k].lock, NULL);
        if (!strstr(ops[k].name, filter)) continue;
        run_directed(k);
        if (per_op) parallel(k, random_worker);
        if (exhaustive && ops[k].arity == 1 && (ops[k].in == F32 || ops[k].in == I32 || ops[k].in == U32))
            parallel(k, exhaustive_worker);
    }
    int unexpected = 0;
    printf("%-16s %10s %12s %12s  mismatches by class (directed/random/exhaustive)\n", "op", "directed",
           "random", "exhaustive");
    for (int k = 0; k < NOPS; k++) {
        Stats *st = &stats[k];
        printf("%-16s %10lu %12lu %12lu ", ops[k].name, (unsigned long)st->compared[0],
               (unsigned long)st->compared[1], (unsigned long)st->compared[2]);
        int any = 0;
        for (int c = 0; c < NCLASSES; c++) {
            unsigned long n0 = st->count[0][c], n1 = st->count[1][c], n2 = st->count[2][c];
            if (!(n0 | n1 | n2)) continue;
            int ok = is_expected(ops[k].name, c);
            unexpected += !ok;
            printf(" %s%s=%lu/%lu/%lu", ok ? "" : "UNEXPECTED:", class_names[c], n0, n1, n2);
            any = 1;
        }
        printf("%s\n", any ? "" : " ok");
        for (int c = 0; c < NCLASSES; c++)
            if (st->have_example[c]) printf("    %s e.g. %s\n", class_names[c], st->example[c]);
    }
    out_of_range_notes();
    printf("unexpected mismatch classes: %d\n", unexpected);
    (void)fmt_names;
    return unexpected > 100 ? 100 : unexpected;
}
