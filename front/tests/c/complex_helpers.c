/* complex_helpers.c: libgcc's complex multiply and divide helpers (rt/libgcc), called directly and
 * through C's `*` and `/` on _Complex values (clang computes inline and calls the helper when both parts of
 * the inline result are NaN). Operands: the cross product of eight directed values per format, with the
 * cases of C Annex G (zeros of both signs, infinities, NaNs) and the scaling cases (the smallest subnormal,
 * the largest finite value). result(i) is a hash of the bits of the 64 results of group i (one routine, one
 * a, one b, every c and d), with every NaN written as one NaN: which NaN a NaN result is, is not promised
 * (rt/libgcc/README.md). Built natively with -DNATIVE with the default compiler runtime, it prints
 * results() lines. All six helpers are self-contained. */
typedef unsigned long long u64;
typedef unsigned int u32;
typedef unsigned short u16;
#define NOINLINE __attribute__((noinline))

float _Complex __mulsc3(float, float, float, float);
float _Complex __divsc3(float, float, float, float);
double _Complex __muldc3(double, double, double, double);
double _Complex __divdc3(double, double, double, double);
long double _Complex __mulxc3(long double, long double, long double, long double);
long double _Complex __divxc3(long double, long double, long double, long double);

#define ND 8
static volatile u32 DF[ND] = {0x00000000, 0x80000000, 0x3f800000, 0xc0400000,
                              0x00000001, 0x7f7fffff, 0xff800000, 0x7fc00000};
static volatile u64 DD[ND] = {0x0000000000000000ull, 0x8000000000000000ull, 0x3ff0000000000000ull,
                              0xc008000000000000ull, 0x0000000000000001ull, 0x7fefffffffffffffull,
                              0xfff0000000000000ull, 0x7ff8000000000000ull};
static volatile u16 DXE[ND] = {0x0000, 0x8000, 0x3fff, 0xc000, 0x0000, 0x7ffe, 0xffff, 0x7fff};
static volatile u64 DXM[ND] = {0, 0, 0x8000000000000000ull, 0xc000000000000000ull, 1, 0xffffffffffffffffull,
                               0x8000000000000000ull, 0xc000000000000000ull};

static float f_of(int i) { u32 b = DF[i]; float x; __builtin_memcpy(&x, &b, 4); return x; }
static double d_of(int i) { u64 b = DD[i]; double x; __builtin_memcpy(&x, &b, 8); return x; }
static long double x_of(int i) {
  long double x = 0;
  u64 m = DXM[i];
  u16 e = DXE[i];
  __builtin_memcpy(&x, &m, 8);
  __builtin_memcpy((char *)&x + 8, &e, 2);
  return x;
}

static u64 mix(u64 h, u64 v) { return (h ^ v) * 0x100000001b3ull; }
static u64 hf(u64 h, float x) {
  u32 b;
  __builtin_memcpy(&b, &x, 4);
  return mix(h, x != x ? 0x7fc00000 : b);
}
static u64 hd(u64 h, double x) {
  u64 b;
  __builtin_memcpy(&b, &x, 8);
  return mix(h, x != x ? 0x7ff8000000000000ull : b);
}
static u64 hx(u64 h, long double x) {
  u64 m = 0;
  u16 e = 0;
  if (x != x) {
    m = 0xc000000000000000ull;
    e = 0x7fff;
  } else {
    __builtin_memcpy(&m, &x, 8);
    __builtin_memcpy(&e, (char *)&x + 8, 2);
  }
  return mix(mix(h, m), e);
}

#define ROUTINES 12
#define GROUPS (ND * ND)

/* The hash of routine r on a = value p, b = value q, every c and d. */
NOINLINE u64 group(int r, int p, int q) {
  u64 h = 0xcbf29ce484222325ull;
  for (int s = 0; s < ND; s++) {
    for (int t = 0; t < ND; t++) {
      if (r < 4) {
        float a = f_of(p), b = f_of(q), c = f_of(s), d = f_of(t);
        float _Complex x, y;
        __real__ x = a; __imag__ x = b; __real__ y = c; __imag__ y = d;
        float _Complex z = r == 0 ? __mulsc3(a, b, c, d) : r == 1 ? __divsc3(a, b, c, d) : r == 2 ? x * y : x / y;
        h = hf(hf(h, __real__ z), __imag__ z);
      } else if (r < 8) {
        double a = d_of(p), b = d_of(q), c = d_of(s), d = d_of(t);
        double _Complex x, y;
        __real__ x = a; __imag__ x = b; __real__ y = c; __imag__ y = d;
        double _Complex z = r == 4 ? __muldc3(a, b, c, d) : r == 5 ? __divdc3(a, b, c, d) : r == 6 ? x * y : x / y;
        h = hd(hd(h, __real__ z), __imag__ z);
      } else {
        long double a = x_of(p), b = x_of(q), c = x_of(s), d = x_of(t);
        long double _Complex x, y;
        __real__ x = a; __imag__ x = b; __real__ y = c; __imag__ y = d;
        long double _Complex z = r == 8 ? __mulxc3(a, b, c, d) : r == 9 ? __divxc3(a, b, c, d) : r == 10 ? x * y : x / y;
        h = hx(hx(h, __real__ z), __imag__ z);
      }
    }
  }
  return h;
}

unsigned results(void) { return ROUTINES * GROUPS; }

u64 result(unsigned i) { return group(i / GROUPS, i % GROUPS / ND, i % ND); }

#ifdef NATIVE
int printf(const char *, ...);
int main(void) {
  for (unsigned i = 0; i < results(); i++) {
    printf("%llu\n", result(i));
  }
  return 0;
}
#endif
