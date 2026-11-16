/* Noncanonical x87 operands through the device's floating-point runtime (rt/softfloat/libgcc.c), against
 * the native program, bit for bit. The runtime is the device's `float-runtime.bc` compiled natively with
 * every symbol prefixed `rt_` (front/tests/tests/softfloat.rs), as for float_runtime_exact.c.
 *
 * Each operand class x87 reads its own way, several encodings each: a pseudo-denormal (exponent 0, integer
 * bit set; read as the denormal of the same value), and the unsupported encodings, an exponent other than
 * 0 with the integer bit clear (an unnormal, an unnormal zero, a pseudo-infinity, a pseudo-NaN; invalid
 * operands, giving the indefinite). Each meets canonical partners (zeros, ones, the smallest normal and
 * subnormal, the largest finite, infinities, a quiet and a signalling NaN) on both sides of every two-operand
 * operation.
 *
 * Where the native program runs x87 instructions (arithmetic, comparisons, conversions, sqrt, rounding to an
 * integral value, glibc's x87 fminl and fmaxl), every result bit must agree, NaNs included. Three routines
 * are software natively and read encodings at face value, so they differ there by design (`library`):
 * glibc's fmal and roundl, and libgcc's __truncxfhf2 (there is no x87 conversion to _Float16).
 * The exit status is the number of mismatches outside `library`, capped at 100. */

#include <math.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long long u64;
typedef unsigned short u16;
typedef long cmp_t;
typedef __int128 ti;
typedef unsigned __int128 uti;

long double rt___addxf3(long double, long double), rt___subxf3(long double, long double),
    rt___mulxf3(long double, long double), rt___divxf3(long double, long double);
cmp_t rt___eqxf2(long double, long double), rt___nexf2(long double, long double),
    rt___ltxf2(long double, long double), rt___lexf2(long double, long double), rt___gtxf2(long double, long double),
    rt___gexf2(long double, long double), rt___unordxf2(long double, long double);
float rt___truncxfsf2(long double);
double rt___truncxfdf2(long double);
_Float16 rt___truncxfhf2(long double);
int rt___fixxfsi(long double);
long long rt___fixxfdi(long double);
unsigned rt___fixunsxfsi(long double);
unsigned long long rt___fixunsxfdi(long double);
ti rt___fixxfti(long double);
uti rt___fixunsxfti(long double);
long double rt_sqrtl(long double), rt_floorl(long double), rt_ceill(long double), rt_truncl(long double),
    rt_roundl(long double), rt_rintl(long double), rt_nearbyintl(long double), rt___powixf2(long double, int),
    rt_fminl(long double, long double), rt_fmaxl(long double, long double),
    rt_fmal(long double, long double, long double);

static long double x80(u16 e, u64 m) {
  long double x = 0;
  memcpy(&x, &m, 8);
  memcpy((char *)&x + 8, &e, 2);
  return x;
}

enum { PSEUDO_DENORMAL, UNNORMAL, UNNORMAL_ZERO, PSEUDO_INFINITY, PSEUDO_NAN, NCLASSES };
static const char *const class_names[NCLASSES] = {"pseudo-denormal", "unnormal", "unnormal-zero",
                                                  "pseudo-infinity", "pseudo-nan"};
static const struct {
  int cls;
  u16 e;
  u64 m;
} odd[] = {
    {PSEUDO_DENORMAL, 0x0000, 0x8000000000000000ull}, {PSEUDO_DENORMAL, 0x8000, 0x8000000000000001ull},
    {PSEUDO_DENORMAL, 0x0000, 0xFFFFFFFFFFFFFFFFull}, {PSEUDO_DENORMAL, 0x0000, 0xC000000000000000ull},
    {UNNORMAL, 0x3fe0, 0x0086686f0f664c3full},        {UNNORMAL, 0x4040, 0x0000000000000001ull},
    {UNNORMAL, 0xbfff, 0x4000000000000000ull},        {UNNORMAL_ZERO, 0x3fff, 0},
    {UNNORMAL_ZERO, 0x8001, 0},                       {PSEUDO_INFINITY, 0x7fff, 0},
    {PSEUDO_INFINITY, 0xffff, 0},                     {PSEUDO_NAN, 0x7fff, 0x4000000000000000ull},
    {PSEUDO_NAN, 0xffff, 0x0000000000000001ull},      {PSEUDO_NAN, 0x7fff, 0x7fffffffffffffffull},
};
/* +0, -0, 1, -1, smallest normal, smallest subnormal, largest finite, +inf, -inf, a quiet NaN, a signalling
 * NaN, 2^63, 0.75, a normal one ulp above the smallest. */
static const struct {
  u16 e;
  u64 m;
} partner[] = {{0, 0},
               {0x8000, 0},
               {0x3fff, 0x8000000000000000ull},
               {0xbfff, 0x8000000000000000ull},
               {0x0001, 0x8000000000000000ull},
               {0x0000, 1},
               {0x7ffe, 0xFFFFFFFFFFFFFFFFull},
               {0x7fff, 0x8000000000000000ull},
               {0xffff, 0x8000000000000000ull},
               {0x7fff, 0xC000000000000001ull},
               {0x7fff, 0x8000000000000001ull},
               {0x403e, 0x8000000000000000ull},
               {0x3ffe, 0xC000000000000000ull},
               {0x0001, 0x8000000000000001ull}};
#define N(a) (int)(sizeof(a) / sizeof *(a))

enum { MAXOPS = 40 };
static const char *op_names[MAXOPS];
static int op_library[MAXOPS], tried[MAXOPS][NCLASSES], missed[MAXOPS][NCLASSES];
static volatile long double va, vb;

static void record(int op, const char *name, int library, int cls, const void *native, const void *device,
                   size_t bytes) {
  op_names[op] = name;
  op_library[op] = library;
  tried[op][cls]++;
  if (memcmp(native, device, bytes)) missed[op][cls]++;
}
/* Both long double results' 10 bytes; the 6 bytes of padding are not part of the value. */
#define LD(op, name, native, device)                                                                       \
  do {                                                                                                     \
    long double n_ = (native), d_ = (device);                                                              \
    record(op, name, 0, cls, &n_, &d_, 10);                                                                \
  } while (0)
#define LIB_LD(op, name, native, device)                                                                   \
  do {                                                                                                     \
    long double n_ = (native), d_ = (device);                                                              \
    record(op, name, 1, cls, &n_, &d_, 10);                                                                \
  } while (0)
#define VAL(op, name, library, T, native, device)                                                          \
  do {                                                                                                     \
    T n_ = (native), d_ = (device);                                                                        \
    record(op, name, library, cls, &n_, &d_, sizeof n_);                                                   \
  } while (0)

int main(void) {
  for (int o = 0; o < N(odd); o++) {
    int cls = odd[o].cls;
    long double v = x80(odd[o].e, odd[o].m);
    for (int k = 0; k < N(partner); k++) {
      for (int side = 0; side < 2; side++) {
        long double w = x80(partner[k].e, partner[k].m), x = side ? w : v, y = side ? v : w;
        va = x;
        vb = y;
        LD(0, "add", va + vb, rt___addxf3(x, y));
        LD(1, "sub", va - vb, rt___subxf3(x, y));
        LD(2, "mul", va * vb, rt___mulxf3(x, y));
        LD(3, "div", va / vb, rt___divxf3(x, y));
        VAL(4, "lt", 0, int, va < vb, rt___ltxf2(x, y) < 0);
        VAL(5, "le", 0, int, va <= vb, rt___lexf2(x, y) <= 0);
        VAL(6, "gt", 0, int, va > vb, rt___gtxf2(x, y) > 0);
        VAL(7, "ge", 0, int, va >= vb, rt___gexf2(x, y) >= 0);
        VAL(8, "eq", 0, int, va == vb, rt___eqxf2(x, y) == 0);
        VAL(9, "ne", 0, int, va != vb, rt___nexf2(x, y) != 0);
        VAL(10, "unord", 0, int, __builtin_isunordered(va, vb), rt___unordxf2(x, y) != 0);
        LD(11, "fminl", fminl(va, vb), rt_fminl(x, y));
        LD(12, "fmaxl", fmaxl(va, vb), rt_fmaxl(x, y));
        LIB_LD(13, "fmal-xyy", fmal(va, vb, vb), rt_fmal(x, y, y));
        LIB_LD(14, "fmal-yyx", fmal(vb, vb, va), rt_fmal(y, y, x));
      }
    }
    va = v;
    VAL(15, "to-f32", 0, float, (float)va, rt___truncxfsf2(v));
    VAL(16, "to-f64", 0, double, (double)va, rt___truncxfdf2(v));
    VAL(17, "to-i32", 0, int, (int)va, rt___fixxfsi(v));
    VAL(18, "to-i64", 0, long long, (long long)va, rt___fixxfdi(v));
    VAL(19, "to-u32", 0, unsigned, (unsigned)va, rt___fixunsxfsi(v));
    VAL(20, "to-u64", 0, unsigned long long, (unsigned long long)va, rt___fixunsxfdi(v));
    VAL(21, "to-i128", 0, ti, (ti)va, rt___fixxfti(v));
    VAL(22, "to-u128", 0, uti, (uti)va, rt___fixunsxfti(v));
    VAL(23, "to-f16", 1, _Float16, (_Float16)va, rt___truncxfhf2(v));
    LD(24, "sqrtl", sqrtl(va), rt_sqrtl(v));
    LD(25, "floorl", floorl(va), rt_floorl(v));
    LD(26, "ceill", ceill(va), rt_ceill(v));
    LD(27, "truncl", truncl(va), rt_truncl(v));
    LIB_LD(28, "roundl", roundl(va), rt_roundl(v));
    LD(29, "rintl", rintl(va), rt_rintl(v));
    LD(30, "nearbyintl", nearbyintl(va), rt_nearbyintl(v));
    LD(31, "powi3", __builtin_powil(va, 3), rt___powixf2(v, 3));
  }
  int unexpected = 0;
  printf("operation: mismatches/tried per class (");
  for (int c = 0; c < NCLASSES; c++) printf(c ? ", %s" : "%s", class_names[c]);
  printf(")\n");
  for (int op = 0; op < MAXOPS; op++) {
    if (!op_names[op]) continue;
    int bad = 0;
    printf("%s", op_names[op]);
    for (int c = 0; c < NCLASSES; c++) {
      printf(" %d/%d", missed[op][c], tried[op][c]);
      bad += missed[op][c];
    }
    printf(op_library[op] ? " library\n" : bad ? " MISMATCH\n" : " ok\n");
    if (!op_library[op]) unexpected += bad;
  }
  printf("unexpected mismatches: %d\n", unexpected);
  return unexpected > 100 ? 100 : unexpected;
}
