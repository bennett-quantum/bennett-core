/* The floating-point runtime's own routines (rt/softfloat/libgcc.c over Berkeley SoftFloat 3e) against what
 * the native program calls for the same operation, bit for bit: those that are more than a SoftFloat call
 * (softfloat_exact.c checks SoftFloat itself against the hardware). The runtime is the device's own,
 * `float-runtime.bc`, compiled natively with every symbol prefixed `rt_` (front/tests/tests/softfloat.rs),
 * so that `rt___fixdfti` is the runtime's and `__fixdfti` libgcc's.
 *
 * The native side is the C operation compiled by clang 18 for x86-64: a call of libgcc's routine (GCC 13's
 * libgcc, which clang links), of glibc's (fmal), or SSE and x87 instructions. Where clang 18 cannot compile
 * the operation (its back end stops with "Unsupported library call operation" for long double to __bf16),
 * the native side is the libgcc routine a GCC build calls, called directly.
 *
 * Usage: float_runtime_exact [random-per-op [threads [exhaustive [name-filter]]]]
 *   The defaults (a fixed-seed sample, one thread) are the regression test; with exhaustive set, every
 *   32-bit input of each operation from float is tried as well. Every 16-bit input of each operation from
 *   _Float16 is always tried.
 *
 * A mismatch is `value` unless an operand is an extF80 encoding that x87 does not accept or reads otherwise
 * (pseudo-denormal, unnormal, pseudo-infinity, pseudo-NaN), which no arithmetic produces (`noncanonical`).
 * The runtime reads those as x87 does, so they must agree where the native side runs x87 instructions; they
 * are expected to differ only where the native side is software reading them at face value (`software`
 * below). The exit status is the number of unexpected classes, capped at 100. */

#include <math.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned __int128 U;
typedef __int128 S;
typedef unsigned long long u64;

#define NOINLINE __attribute__((noinline))

/* ---- the runtime, prefixed ---------------------------------------------------------------------- */

S rt___fixsfti(float);
U rt___fixunssfti(float);
S rt___fixdfti(double);
U rt___fixunsdfti(double);
S rt___fixxfti(long double);
U rt___fixunsxfti(long double);
float rt___floattisf(S);
float rt___floatuntisf(U);
double rt___floattidf(S);
double rt___floatuntidf(U);
long double rt___floattixf(S);
long double rt___floatuntixf(U);
long double rt_fmal(long double, long double, long double);
float rt___extendhfsf2(_Float16);
long double rt___extendhfxf2(_Float16);
_Float16 rt___truncsfhf2(float);
_Float16 rt___truncdfhf2(double);
_Float16 rt___truncxfhf2(long double);
S rt___fixhfti(_Float16);
U rt___fixunshfti(_Float16);
_Float16 rt___floattihf(S);
_Float16 rt___floatuntihf(U);
__bf16 rt___truncsfbf2(float);
__bf16 rt___truncdfbf2(double);
__bf16 rt___truncxfbf2(long double);
/* What a GCC build calls where clang 18 cannot compile the operation. */
__bf16 __truncxfbf2(long double);

/* ---- formats and bits --------------------------------------------------------------------------- */

enum fmt { F16, BF16, F32, F64, X80, I128, U128 };
static const char *const fmt_names[] = {"f16", "bf16", "f32", "f64", "f80", "i128", "u128"};

static _Float16 h16(U v) { unsigned short u = (unsigned short)v; _Float16 f; memcpy(&f, &u, 2); return f; }
static __bf16 b16(U v) { unsigned short u = (unsigned short)v; __bf16 f; memcpy(&f, &u, 2); return f; }
static U ofh16(_Float16 f) { unsigned short u; memcpy(&u, &f, 2); return u; }
static U ofb16(__bf16 f) { unsigned short u; memcpy(&u, &f, 2); return u; }

static float f32(U v) { unsigned u = (unsigned)v; float f; memcpy(&f, &u, 4); return f; }
static double f64(U v) { u64 u = (u64)v; double d; memcpy(&d, &u, 8); return d; }
/* long double: the significand (with the integer bit) in the low 64 bits, sign and exponent above. */
static long double x80(U v) {
    unsigned char b[16] = {0};
    u64 m = (u64)v;
    unsigned short e = (unsigned short)(v >> 64);
    memcpy(b, &m, 8);
    memcpy(b + 8, &e, 2);
    long double x;
    memcpy(&x, b, 16);
    return x;
}
static U of32(float f) { unsigned u; memcpy(&u, &f, 4); return u; }
static U of64(double d) { u64 u; memcpy(&u, &d, 8); return u; }
static U of80(long double x) {
    u64 m;
    unsigned short e;
    memcpy(&m, &x, 8);
    memcpy(&e, (const char *)&x + 8, 2);
    return (U)e << 64 | m;
}

static int pseudo_denormal(U v) { return (v >> 64 & 0x7FFF) == 0 && (u64)v >> 63; }
static int unsupported(U v) { return (v >> 64 & 0x7FFF) != 0 && !((u64)v >> 63); }
static int noncanonical(int f, U v) { return f == X80 && (pseudo_denormal(v) || unsupported(v)); }

static U mask(int f, U v) {
    switch (f) {
    case F16: case BF16: return (unsigned short)v;
    case F32: return (unsigned)v;
    case F64: return (u64)v;
    case X80: return v & (((U)1 << 80) - 1);
    default: return v;
    }
}

static void show(char *out, size_t n, int f, U v) {
    switch (f) {
    case F16: case BF16: snprintf(out, n, "%04x", (unsigned)(unsigned short)v); break;
    case F32: snprintf(out, n, "%08x", (unsigned)v); break;
    case F64: snprintf(out, n, "%016llx", (u64)v); break;
    case X80: snprintf(out, n, "%04x:%016llx", (unsigned)(v >> 64 & 0xFFFF), (u64)v); break;
    default: snprintf(out, n, "%016llx%016llx", (u64)(v >> 64), (u64)v); break;
    }
}

/* ---- the operations: native (h) and runtime (s) ------------------------------------------------- */

typedef U (*fn)(U, U, U);

#define UNARY(NAME, HEXPR, SEXPR)                                                     \
    static NOINLINE U h_##NAME(U a, U b, U c) { (void)b; (void)c; return HEXPR; }      \
    static NOINLINE U s_##NAME(U a, U b, U c) { (void)b; (void)c; return SEXPR; }

/* To 128-bit integers. */
UNARY(f32_i128, (U)(S)f32(a), (U)rt___fixsfti(f32(a)))
UNARY(f32_u128, (U)f32(a), rt___fixunssfti(f32(a)))
UNARY(f64_i128, (U)(S)f64(a), (U)rt___fixdfti(f64(a)))
UNARY(f64_u128, (U)f64(a), rt___fixunsdfti(f64(a)))
UNARY(x80_i128, (U)(S)x80(a), (U)rt___fixxfti(x80(a)))
UNARY(x80_u128, (U)x80(a), rt___fixunsxfti(x80(a)))
/* From 128-bit integers. */
UNARY(i128_f32, of32((float)(S)a), of32(rt___floattisf((S)a)))
UNARY(u128_f32, of32((float)a), of32(rt___floatuntisf(a)))
UNARY(i128_f64, of64((double)(S)a), of64(rt___floattidf((S)a)))
UNARY(u128_f64, of64((double)a), of64(rt___floatuntidf(a)))
UNARY(i128_x80, of80((long double)(S)a), of80(rt___floattixf((S)a)))
UNARY(u128_x80, of80((long double)a), of80(rt___floatuntixf(a)))

/* The fused multiply-add of long double: glibc's fmal, which the native program calls. */
static NOINLINE U h_x80_fma(U a, U b, U c) { return of80(fmal(x80(a), x80(b), x80(c))); }
static NOINLINE U s_x80_fma(U a, U b, U c) { return of80(rt_fmal(x80(a), x80(b), x80(c))); }

/* _Float16 and __bf16. */
UNARY(f16_f32, of32((float)h16(a)), of32(rt___extendhfsf2(h16(a))))
UNARY(f16_x80, of80((long double)h16(a)), of80(rt___extendhfxf2(h16(a))))
UNARY(f32_f16, ofh16((_Float16)f32(a)), ofh16(rt___truncsfhf2(f32(a))))
UNARY(f64_f16, ofh16((_Float16)f64(a)), ofh16(rt___truncdfhf2(f64(a))))
UNARY(x80_f16, ofh16((_Float16)x80(a)), ofh16(rt___truncxfhf2(x80(a))))
UNARY(f16_i128, (U)(S)h16(a), (U)rt___fixhfti(h16(a)))
UNARY(f16_u128, (U)h16(a), rt___fixunshfti(h16(a)))
UNARY(i128_f16, ofh16((_Float16)(S)a), ofh16(rt___floattihf((S)a)))
UNARY(u128_f16, ofh16((_Float16)a), ofh16(rt___floatuntihf(a)))
UNARY(f32_bf16, ofb16((__bf16)f32(a)), ofb16(rt___truncsfbf2(f32(a))))
UNARY(f64_bf16, ofb16((__bf16)f64(a)), ofb16(rt___truncdfbf2(f64(a))))
UNARY(x80_bf16, ofb16(__truncxfbf2(x80(a))), ofb16(rt___truncxfbf2(x80(a))))

typedef struct {
    const char *name;
    int arity, in, out;
    fn h, s;
    /* The native side is software that reads long double encodings at face value (glibc's fmal, libgcc's
     * truncations to _Float16 and __bf16), where the runtime reads them as x87 does: noncanonical operands
     * are expected to differ. Elsewhere the native side runs x87 instructions and they must agree. */
    int software;
} Op;

#define OP(NAME, AR, IN, OUT) {#NAME, AR, IN, OUT, h_##NAME, s_##NAME, 0}
#define SOFTWARE_OP(NAME, AR, IN, OUT) {#NAME, AR, IN, OUT, h_##NAME, s_##NAME, 1}
static const Op ops[] = {
    OP(f32_i128, 1, F32, I128), OP(f32_u128, 1, F32, U128), OP(f64_i128, 1, F64, I128),
    OP(f64_u128, 1, F64, U128), OP(x80_i128, 1, X80, I128), OP(x80_u128, 1, X80, U128),
    OP(i128_f32, 1, I128, F32), OP(u128_f32, 1, U128, F32), OP(i128_f64, 1, I128, F64),
    OP(u128_f64, 1, U128, F64), OP(i128_x80, 1, I128, X80), OP(u128_x80, 1, U128, X80),
    SOFTWARE_OP(x80_fma, 3, X80, X80),
    OP(f16_f32, 1, F16, F32), OP(f16_x80, 1, F16, X80), OP(f32_f16, 1, F32, F16), OP(f64_f16, 1, F64, F16),
    SOFTWARE_OP(x80_f16, 1, X80, F16), OP(f16_i128, 1, F16, I128), OP(f16_u128, 1, F16, U128),
    OP(i128_f16, 1, I128, F16), OP(u128_f16, 1, U128, F16),
    OP(f32_bf16, 1, F32, BF16), OP(f64_bf16, 1, F64, BF16), SOFTWARE_OP(x80_bf16, 1, X80, BF16),
};
#define NOPS ((int)(sizeof ops / sizeof ops[0]))

/* ---- operands --------------------------------------------------------------------------------- */

static inline u64 next(u64 *s) { /* splitmix64 */
    u64 z = (*s += 0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EB;
    return z ^ (z >> 31);
}

/* Field widths: significand bits stored, exponent bits. */
static const int sig_bits[] = {10, 7, 23, 52, 64}, exp_bits[] = {5, 8, 8, 11, 15};
static int bias(int f) { return (1 << (exp_bits[f] - 1)) - 1; }

static U pack(int f, unsigned sign, unsigned exp, u64 sig) {
    switch (f) {
    case F32: return (U)sign << 31 | (U)exp << 23 | (sig & 0x7FFFFF);
    case F64: return (U)sign << 63 | (U)exp << 52 | (sig & 0xFFFFFFFFFFFFF);
    default: /* canonical: the integer bit set exactly when the exponent is not zero */
        return (U)(sign << 15 | exp) << 64 | (exp ? sig | 0x8000000000000000 : sig & 0x7FFFFFFFFFFFFFFF);
    }
}

#define MAXD 200
static U directed[5][MAXD];
static int ndirected[5];
static void add(int f, U v) { if (ndirected[f] < MAXD) directed[f][ndirected[f]++] = v; }

/* Values at the edges of the 64- and 128-bit ranges, and the usual specials, of every format. */
static void build_directed(void) {
    static const long double edges[] = {
        0, 0.25L, 0.5L, 0.75L, 1, 1.5L, 2, 3, 0x1p23L, 0x1p24L, 0x1p52L, 0x1p53L, 0x1p62L,
        0x1p63L, 0x1.fffffffffffffffep62L, 0x1p64L, 0x1.fffffffffffffffep63L, 0x1.0000000000000002p64L,
        0x1p65L, 0x1p100L, 0x1p126L, 0x1.fffffffffffffffep126L, 0x1p127L, 0x1.0000000000000002p127L,
        0x1.fffffffffffffffep127L, 0x1p128L, 0x1.8p127L, 0x1p129L, 0x1p200L, 1e30L, 1e300L,
        0x1.fffffep126L, 0x1.fffffffffffffp126L, 0x1.000002p127L, 0x1.0000000000001p127L,
    };
    for (unsigned sign = 0; sign < 2; sign++) {
        for (size_t i = 0; i < sizeof edges / sizeof edges[0]; i++) {
            long double x = sign ? -edges[i] : edges[i];
            add(F32, of32((float)x));
            add(F64, of64((double)x));
            add(X80, of80(x));
        }
        /* subnormals, the least normal, the greatest finite, inf, quiet and signalling NaNs */
        static const unsigned e32[] = {0, 0, 1, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF};
        static const u64 m32[] = {1, 0x7FFFFF, 0, 0x7FFFFF, 0, 0x400000, 0x400123, 0x000456};
        static const unsigned e64[] = {0, 0, 1, 0x7FE, 0x7FF, 0x7FF, 0x7FF, 0x7FF};
        static const u64 m64[] = {1, 0xFFFFFFFFFFFFF, 0, 0xFFFFFFFFFFFFF, 0, 0x8000000000000, 0x8000000000123, 0x456};
        static const unsigned e80[] = {0, 0, 1, 0x7FFE, 0x7FFF, 0x7FFF, 0x7FFF, 0x7FFF};
        static const u64 m80[] = {1, 0x7FFFFFFFFFFFFFFF, 0, 0xFFFFFFFFFFFFFFFF, 0, 0x4000000000000000,
                                  0x4000000000000123, 0x456};
        for (int i = 0; i < 8; i++) {
            add(F32, pack(F32, sign, e32[i], m32[i]));
            add(F64, pack(F64, sign, e64[i], m64[i]));
            add(X80, pack(X80, sign, e80[i], m80[i]));
        }
        /* non-canonical extF80: pseudo-denormal, unnormal, pseudo-infinity, pseudo-NaN */
        add(X80, (U)(sign << 15) << 64 | 0x8000000000000001);
        add(X80, (U)(sign << 15 | 0x403F) << 64 | 0x4000000000000000);
        add(X80, (U)(sign << 15 | 0x7FFF) << 64);
        add(X80, (U)(sign << 15 | 0x7FFF) << 64 | 0x4000000000000000);
    }
}

/* 128-bit integers whose conversions round: around each power of two, and ties at the rounding position of
 * each format (24, 53, 64 bits), just below, at, and just above. */
static U random_int(u64 *s) {
    u64 r = next(s);
    U x = (U)next(s) << 64 | next(s);
    switch (r % 8) {
    case 0: return x;
    case 1: case 2: return x >> (r >> 8) % 128;
    case 3: return -(x >> (r >> 8) % 128);
    case 4: return ((U)1 << (r >> 8) % 128) + (U)((r >> 16) % 5) - 2;
    default: { /* a tie at precision p, perturbed */
        static const int p[] = {24, 53, 64};
        int bits = p[(r >> 8) % 3], top = bits + (int)((r >> 16) % (unsigned)(128 - bits));
        U v = (U)1 << top | (x & (((U)1 << top) - 1) & ~(((U)1 << (top - bits + 1)) - 1));
        v |= (U)1 << (top - bits); /* the half */
        int d = (int)((r >> 24) % 3) - 1;
        if (d) v += (U)(S)d;
        if (top - bits > 1 && (r >> 28 & 1)) v |= 1; /* a sticky bit */
        return (r >> 30 & 1) ? -v : v;
    }
    }
}

static U random_float(int f, u64 *s) {
    u64 r = next(s), sig = next(s);
    if (f <= BF16) return (unsigned short)r;
    unsigned emax = (1u << exp_bits[f]) - 1, sign = (unsigned)(r & 1);
    switch ((r >> 8) % 8) {
    case 0: /* uniform bits (for extF80: a canonical encoding) */
        if (f == F32) return (unsigned)sig;
        if (f == F64) return sig;
        return pack(f, sign, (unsigned)(r >> 16) & 0x7FFF, sig);
    case 1: case 2: case 3: /* 2^-2 to 2^130: the conversions' range and around it */
        return pack(f, sign, (unsigned)(bias(f) - 2 + (int)((r >> 16) % 133)), sig);
    case 4: /* tiny exponents */
        return pack(f, sign, (unsigned)((r >> 16) % (unsigned)(sig_bits[f] + 3)), sig);
    case 5: /* near overflow */
        return pack(f, sign, emax - 1 - (unsigned)((r >> 16) % 4), sig);
    case 6: /* 2^-170 to 2^20: the ranges of _Float16 and float, subnormals included */
        if (f != F32) return pack(f, sign, (unsigned)(bias(f) - 170 + (int)((r >> 16) % 191)), sig);
        return pack(f, sign, (unsigned)((r >> 16) % 148), sig);
    default: /* a directed value, a few ulps off */
        return mask(f, directed[f][(r >> 16) % (unsigned)ndirected[f]] + (U)((r >> 24) % 5) - 2);
    }
}

static U random_operand(int f, u64 *s) { return f >= I128 ? random_int(s) : random_float(f, s); }

static unsigned exp80(U v) { return (unsigned)(v >> 64) & 0x7FFF; }
static U with_exp80(U v, int e) {
    if (e < 1) e = 1;
    if (e > 0x7FFE) e = 0x7FFE;
    return (U)((unsigned)(v >> 64 & 0x8000) | (unsigned)e) << 64 | ((u64)v | 0x8000000000000000);
}

/* Operands of a * b + c: the product's exponent sometimes steered to the subnormal range or to overflow, the
 * addend near the product (cancellation), its negated rounding a few ulps off (cancellation of nearly every
 * bit), or anything. */
static void random_fma(u64 *s, U *a, U *b, U *c) {
    u64 r = next(s);
    *a = random_float(X80, s);
    *b = random_float(X80, s);
    switch ((r >> 4) % 4) {
    case 0: { /* product near the subnormal range */
        int ea = 1 + (int)((r >> 8) % 0x3FFF);
        *a = with_exp80(*a, ea);
        *b = with_exp80(*b, 0x3FFF - ea + (int)((r >> 24) % 140) - 70);
        break;
    }
    case 1: { /* product near overflow */
        int ea = 0x3FFF + (int)((r >> 8) % 0x3FFF);
        *a = with_exp80(*a, ea);
        *b = with_exp80(*b, 0x7FFE + 0x3FFF - ea - (int)((r >> 24) % 4));
        break;
    }
    default: break;
    }
    int pe = (int)exp80(*a) + (int)exp80(*b) - 0x3FFF;
    switch ((r >> 32) % 6) {
    case 0: *c = random_float(X80, s); break;
    case 1: case 2: *c = with_exp80(random_float(X80, s), pe + (int)((r >> 40) % 141) - 70); break;
    case 3: case 4: { /* minus the rounded product, a few ulps off, maybe a few binades lower */
        U p = of80(x80(*a) * x80(*b));
        U q = (p ^ ((U)0x8000 << 64)) + (U)((r >> 40) % 9) - 4;
        if ((r >> 48) & 1) q = with_exp80(q, (int)exp80(q) - (int)((r >> 50) % 66));
        *c = noncanonical(X80, q) ? p : q;
        break;
    }
    default: *c = random_float(X80, s); *c = (U)(*c >> 64 & 0x8000) << 64; break; /* a zero */
    }
}

/* ---- comparison and bookkeeping --------------------------------------------------------------- */

enum { VALUE, NONCANONICAL, NCLASSES };
static const char *const class_names[] = {"value", "noncanonical"};

typedef struct {
    atomic_ulong compared[3]; /* directed, random, exhaustive */
    atomic_ulong count[3][NCLASSES];
    pthread_mutex_t lock;
    int have_example[NCLASSES];
    char example[NCLASSES][240];
} Stats;
static Stats stats[NOPS];

typedef struct {
    unsigned long compared, count[NCLASSES];
} Local;

static void merge(int k, int phase, const Local *l) {
    atomic_fetch_add(&stats[k].compared[phase], l->compared);
    for (int c = 0; c < NCLASSES; c++) atomic_fetch_add(&stats[k].count[phase][c], l->count[c]);
}

static void check(int k, Local *l, U a, U b, U c) {
    const Op *op = &ops[k];
    U h = mask(op->out, op->h(a, b, c)), s = mask(op->out, op->s(a, b, c));
    l->compared++;
    if (h == s) return;
    int odd = noncanonical(op->in, a) || (op->arity > 1 && noncanonical(op->in, b)) ||
              (op->arity > 2 && noncanonical(op->in, c));
    int cls = odd ? NONCANONICAL : VALUE;
    l->count[cls]++;
    Stats *st = &stats[k];
    if (st->have_example[cls]) return;
    pthread_mutex_lock(&st->lock);
    if (!st->have_example[cls]) {
        char x[3][48], hs[48], ss[48];
        U in[3] = {a, b, c};
        for (int i = 0; i < 3; i++) show(x[i], sizeof x[i], op->in, in[i]);
        show(hs, sizeof hs, op->out, h);
        show(ss, sizeof ss, op->out, s);
        snprintf(st->example[cls], sizeof st->example[cls], "(%s%s%s%s%s) native %s runtime %s", x[0],
                 op->arity > 1 ? ", " : "", op->arity > 1 ? x[1] : "", op->arity > 2 ? ", " : "",
                 op->arity > 2 ? x[2] : "", hs, ss);
        st->have_example[cls] = 1;
    }
    pthread_mutex_unlock(&st->lock);
}

/* ---- the phases ------------------------------------------------------------------------------- */

static const U directed_ints_base[] = {
    0, 1, 2, 3, 0x7FFFFF, 0x1000001, 0x1000003, 0x1FFFFFFFFFFFFF, 0x20000000000001, 0x7FFFFFFFFFFFFFFF,
    0x8000000000000000, 0x8000000000000001, 0xFFFFFFFFFFFFFFFF, (U)1 << 64,
};

static void run_directed(int k) {
    const Op *op = &ops[k];
    Local l = {0};
    if (op->in >= I128) {
        for (size_t i = 0; i < sizeof directed_ints_base / sizeof directed_ints_base[0]; i++)
            for (int d = -1; d <= 1; d++) {
                U v = directed_ints_base[i] + (U)(S)d;
                check(k, &l, v, 0, 0);
                check(k, &l, -v, 0, 0);
            }
        for (int e = 0; e < 128; e++)
            for (int d = -2; d <= 2; d++) {
                U v = ((U)1 << e) + (U)(S)d;
                check(k, &l, v, 0, 0);
                check(k, &l, -v, 0, 0);
                check(k, &l, ~(U)0 >> (127 - e), 0, 0);
            }
        merge(k, 0, &l);
        return;
    }
    int f = op->in, n = ndirected[f];
    if (f <= BF16) /* every input */
        for (U x = 0; x < 0x10000; x++) check(k, &l, x, 0, 0);
    else if (op->arity == 1)
        for (int i = 0; i < n; i++) check(k, &l, directed[f][i], 0, 0);
    else if (op->arity == 2)
        for (int i = 0; i < n; i++)
            for (int j = 0; j < n; j++) check(k, &l, directed[f][i], directed[f][j], 0);
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
    u64 s = 0x5EED0000 + (u64)job->k * 0x10001 + job->t * 0x9E3779B9;
    unsigned long n = per_op / nthreads + (job->t < per_op % nthreads);
    Local l = {0};
    for (unsigned long i = 0; i < n; i++) {
        U a = random_operand(op->in, &s), b = 0, c = 0;
        if (op->arity > 1) b = random_operand(op->in, &s);
        if (op->arity > 2) c = random_operand(op->in, &s);
        if (op->arity == 3 && op->in == X80) random_fma(&s, &a, &b, &c);
        check(job->k, &l, a, b, c);
    }
    merge(job->k, 1, &l);
    return NULL;
}

static void *exhaustive_worker(void *p) {
    const Job *job = p;
    u64 lo = (UINT64_C(1) << 32) * job->t / nthreads, hi = (UINT64_C(1) << 32) * (job->t + 1) / nthreads;
    Local l = {0};
    for (u64 x = lo; x < hi; x++) check(job->k, &l, x, 0, 0);
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

int main(int argc, char **argv) {
    if (argc > 1) per_op = strtoul(argv[1], NULL, 0);
    if (argc > 2) nthreads = strtoul(argv[2], NULL, 0);
    if (argc > 3) exhaustive = atoi(argv[3]);
    const char *filter = argc > 4 ? argv[4] : "";
    if (nthreads < 1) nthreads = 1;
    if (nthreads > 256) nthreads = 256;
    build_directed();
    for (int k = 0; k < NOPS; k++) {
        pthread_mutex_init(&stats[k].lock, NULL);
        if (!strstr(ops[k].name, filter)) continue;
        run_directed(k);
        if (per_op) parallel(k, random_worker);
        if (exhaustive && ops[k].arity == 1 && ops[k].in == F32) parallel(k, exhaustive_worker);
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
            int expected = c == NONCANONICAL && ops[k].software;
            unexpected += !expected;
            printf(" %s%s=%lu/%lu/%lu", expected ? "" : "UNEXPECTED:", class_names[c], n0, n1, n2);
            any = 1;
        }
        printf("%s\n", any ? "" : " ok");
        for (int c = 0; c < NCLASSES; c++)
            if (st->have_example[c]) printf("    %s e.g. %s\n", class_names[c], st->example[c]);
    }
    printf("unexpected mismatch classes: %d\n", unexpected);
    (void)fmt_names;
    return unexpected > 100 ? 100 : unexpected;
}
