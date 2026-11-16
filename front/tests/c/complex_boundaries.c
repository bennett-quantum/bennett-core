/* complex_boundaries.c: libgcc's six complex helpers, called directly and through C's `*` and `/`, on operands
 * whose exponents sit where libgcc's division and multiplication change course: its scaling thresholds RBIG
 * (MAX/2), RMIN (MIN), RMIN2 (EPSILON) and RMAX2 (RBIG*RMIN2) and their neighbours, the overflow and
 * underflow of products, subnormals, ratios whose quotient is subnormal, infinities and NaNs; mantissas
 * zero, all ones, one or random. (From the independent libgcc testing pass; it reaches every line of the
 * helpers' scaling paths.) Prints every result component's raw bits in hex, one per line, natively
 * (-DNATIVE, the default libgcc) and on the device's console. */
typedef unsigned long long u64; typedef unsigned int u32; typedef unsigned short u16;
#ifdef NATIVE
int putchar(int); static void out(char c) { putchar(c); }
#else
void __bennett_effect(char); static void out(char c) { __bennett_effect(c); }
#endif
float _Complex __mulsc3(float, float, float, float); float _Complex __divsc3(float, float, float, float);
double _Complex __muldc3(double, double, double, double); double _Complex __divdc3(double, double, double, double);
long double _Complex __mulxc3(long double, long double, long double, long double);
long double _Complex __divxc3(long double, long double, long double, long double);
#ifndef N
#define N 2000
#endif
static void hex(u64 v, int digits) { for (int i = digits - 1; i >= 0; i--) out("0123456789abcdef"[(v >> (4 * i)) & 15]); }
static void pf(float x) { u32 b; __builtin_memcpy(&b, &x, 4); hex(b, 8); out('\n'); }
static void pd(double x) { u64 b; __builtin_memcpy(&b, &x, 8); hex(b, 16); out('\n'); }
static void px(long double x) { u64 m; u16 e; __builtin_memcpy(&m, &x, 8); __builtin_memcpy(&e, (char *)&x + 8, 2); hex(e, 4); hex(m, 16); out('\n'); }
static u64 state = 0x2545f4914f6cdd1dull;
static u64 next(void) { state ^= state << 13; state ^= state >> 7; state ^= state << 17; return state; }
static u64 mant(u64 r, int bits) {
  u64 all = bits == 64 ? ~0ull : (1ull << bits) - 1;
  switch (r % 6) { case 0: return 0; case 1: return all; case 2: return 1; case 3: return all - 1; default: return next() & all; }
}
/* biased exponents of interest */
static const int EF[] = {0, 1, 2, 127, 126, 128, 127 - 23, 127 - 24, 127 - 25, 254, 253, 255, 127 + 63, 127 + 64, 127 - 63, 127 - 64,
                         127 + 103, 127 + 104, 127 + 105, 127 - 126, 127 - 127, 3, 23, 24, 25, 230, 231, 232};
static const int ED[] = {0, 1, 2, 1023, 1022, 1024, 1023 - 52, 1023 - 53, 1023 - 54, 2046, 2045, 2047, 1023 + 511, 1023 + 512,
                         1023 - 511, 1023 - 512, 1023 + 970, 1023 + 971, 1023 + 972, 1023 - 1022, 1023 - 1023, 3, 52, 53, 54, 1993, 1994, 1995};
static const int EX[] = {0, 1, 2, 16383, 16382, 16384, 16383 - 63, 16383 - 64, 16383 - 65, 32766, 32765, 32767, 16383 + 8191,
                         16383 + 8192, 16383 - 8191, 16383 - 8192, 16383 + 16319, 16383 + 16320, 16383 + 16321, 16383 - 16382, 3, 63, 64, 65,
                         32702, 32703, 32704};
#define NE(a) (int)(sizeof a / sizeof a[0])
static int eint(const int *e, int n) { u64 r = next(); return (r & 7) == 0 ? (int)(next() % (u64)(e == EF ? 256 : e == ED ? 2048 : 32768)) : e[r % (u64)n]; }
static float rf(void) { u64 r = next(); u32 b = (u32)(r >> 63) << 31 | (u32)eint(EF, NE(EF)) << 23 | (u32)mant(r >> 8, 23); float x; __builtin_memcpy(&x, &b, 4); return x; }
static double rd(void) { u64 r = next(); u64 b = (r >> 63) << 63 | (u64)eint(ED, NE(ED)) << 52 | mant(r >> 8, 52); double x; __builtin_memcpy(&x, &b, 8); return x; }
static long double rx(void) {
  u64 r = next(); u16 e = (u16)((r >> 63) << 15 | (u64)eint(EX, NE(EX))); u64 m = mant(r >> 8, 64);
  if ((e & 0x7fff) == 0) m &= ~(1ull << 63); else m |= 1ull << 63;   /* canonical */
  long double x = 0; __builtin_memcpy(&x, &m, 8); __builtin_memcpy((char *)&x + 8, &e, 2); return x;
}
static float _Complex cf(float a, float b) { float _Complex z; __real__ z = a; __imag__ z = b; return z; }
static double _Complex cd(double a, double b) { double _Complex z; __real__ z = a; __imag__ z = b; return z; }
static long double _Complex cx(long double a, long double b) { long double _Complex z; __real__ z = a; __imag__ z = b; return z; }
int main(void) {
  for (int i = 0; i < N; i++) {
    float a = rf(), b = rf(), c = rf(), d = rf();
    float _Complex z;
    z = __mulsc3(a, b, c, d); pf(__real__ z); pf(__imag__ z);
    z = __divsc3(a, b, c, d); pf(__real__ z); pf(__imag__ z);
    z = cf(a, b) * cf(c, d); pf(__real__ z); pf(__imag__ z);
    z = cf(a, b) / cf(c, d); pf(__real__ z); pf(__imag__ z);
  }
  for (int i = 0; i < N; i++) {
    double a = rd(), b = rd(), c = rd(), d = rd();
    double _Complex z;
    z = __muldc3(a, b, c, d); pd(__real__ z); pd(__imag__ z);
    z = __divdc3(a, b, c, d); pd(__real__ z); pd(__imag__ z);
    z = cd(a, b) * cd(c, d); pd(__real__ z); pd(__imag__ z);
    z = cd(a, b) / cd(c, d); pd(__real__ z); pd(__imag__ z);
  }
  for (int i = 0; i < N; i++) {
    long double a = rx(), b = rx(), c = rx(), d = rx();
    long double _Complex z;
    z = __mulxc3(a, b, c, d); px(__real__ z); px(__imag__ z);
    z = __divxc3(a, b, c, d); px(__real__ z); px(__imag__ z);
    z = cx(a, b) * cx(c, d); px(__real__ z); px(__imag__ z);
    z = cx(a, b) / cx(c, d); px(__real__ z); px(__imag__ z);
  }
  return 0;
}
