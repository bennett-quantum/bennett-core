/* Prints the bits of every result of libgcc's complex helpers on directed and random operands, as
 * fixed-width hex (no separators), in the order: mulsc3, divsc3, muldc3, divdc3, mulxc3, divxc3, each
 * first on the cross product of DIRECTED values, then on RANDOM tuples. Natively: -DNATIVE. */
typedef unsigned long long u64;
typedef unsigned int u32;
typedef unsigned short u16;

#ifdef NATIVE
int putchar(int);
static void out(char c) { putchar(c); }
#else
void __bennett_effect(char);
static void out(char c) { __bennett_effect(c); }
#endif

float _Complex __mulsc3(float, float, float, float);
float _Complex __divsc3(float, float, float, float);
double _Complex __muldc3(double, double, double, double);
double _Complex __divdc3(double, double, double, double);
long double _Complex __mulxc3(long double, long double, long double, long double);
long double _Complex __divxc3(long double, long double, long double, long double);

#ifndef RANDOM
#define RANDOM 262144
#endif

static void hex(u64 v, int digits) {
  for (int i = digits - 1; i >= 0; i--) out("0123456789abcdef"[(v >> (4 * i)) & 15]);
}
static void pf(float x) { u32 b; __builtin_memcpy(&b, &x, 4); hex(b, 8); }
static void pd(double x) { u64 b; __builtin_memcpy(&b, &x, 8); hex(b, 16); }
static void px(long double x) {
  u64 m; u16 e;
  __builtin_memcpy(&m, &x, 8);
  __builtin_memcpy(&e, (char *)&x + 8, 2);
  hex(e, 4); hex(m, 16);
}

static float f_of(u32 b) { float x; __builtin_memcpy(&x, &b, 4); return x; }
static double d_of(u64 b) { double x; __builtin_memcpy(&x, &b, 8); return x; }
static long double x_of(u16 e, u64 m) {
  long double x = 0;
  __builtin_memcpy(&x, &m, 8);
  __builtin_memcpy((char *)&x + 8, &e, 2);
  return x;
}

#define ND 20
static u32 DF[ND] = {0x00000000, 0x80000000, 0x3f800000, 0xc0400000, 0x00000001, 0xff7fffff, 0x00800000, 0x7f800000,
                     0xff800000, 0x7fc00000, 0xffc12345, 0x3f400000, 0x7f000000, 0x01800000, 0x5f000000, 0x1f000000, 0x7f800001, 0xff812345, 0x007fffff, 0x80000001};
static u64 DD[ND] = {0x0000000000000000ull, 0x8000000000000000ull, 0x3ff0000000000000ull, 0xc008000000000000ull,
                     0x0000000000000001ull, 0xffefffffffffffffull, 0x0010000000000000ull, 0x7ff0000000000000ull,
                     0xfff0000000000000ull, 0x7ff8000000000000ull, 0xfff8000000012345ull, 0x3fe8000000000000ull,
                     0x7fe0000000000000ull, 0x0030000000000000ull, 0x5fe0000000000000ull, 0x1fe0000000000000ull,
                     0x7ff0000000000001ull, 0xfff0000000012345ull, 0x000fffffffffffffull, 0x8000000000000001ull};
static u16 DXE[ND] = {0x0000, 0x8000, 0x3fff, 0xc000, 0x0000, 0xfffe, 0x0001, 0x7fff,
                      0xffff, 0x7fff, 0xffff, 0x3ffe, 0x7ffe, 0x0003, 0x5fff, 0x1fff, 0x7fff, 0xffff, 0x0000, 0x8000};
static u64 DXM[ND] = {0, 0, 0x8000000000000000ull, 0xc000000000000000ull, 1, 0xffffffffffffffffull,
                      0x8000000000000000ull, 0x8000000000000000ull, 0x8000000000000000ull, 0xc000000000000000ull,
                      0xc000000000012345ull, 0xc000000000000000ull, 0x8000000000000000ull, 0x8000000000000000ull,
                      0x8000000000000000ull, 0x8000000000000000ull,
                      0x8000000000000001ull, 0x8000000000012345ull, 0x7fffffffffffffffull, 1};

static u64 state = 0x9e3779b97f4a7c15ull;
static u64 next(void) {
  state ^= state << 13;
  state ^= state >> 7;
  state ^= state << 17;
  return state;
}

/* Random operands: raw bits, exponents near 1, or a directed value, chosen per operand. */
static u32 rf(void) {
  u64 r = next();
  switch (r & 3) {
  case 0: return (u32)(r >> 32);
  case 1: return (u32)(r >> 32) & 0x807fffff | (u32)(0x7f - 24 + ((r >> 8) & 63)) << 23;
  case 2: return DF[(r >> 8) % ND];
  default: return (u32)(r >> 32) & 0x807fffff | (u32)((r >> 8) & 0xff) << 23;
  }
}
static u64 rd(void) {
  u64 r = next(), s = next();
  switch (r & 3) {
  case 0: return s;
  case 1: return s & 0x800fffffffffffffull | (u64)(0x3ff - 60 + ((r >> 8) & 127)) << 52;
  case 2: return DD[(r >> 8) % ND];
  default: return s & 0x800fffffffffffffull | (u64)((r >> 8) & 0x7ff) << 52;
  }
}
/* Canonical encodings only: the integer bit is set exactly when the exponent is not 0. */
static long double rx(void) {
  u64 r = next(), m = next();
  u16 e;
  switch (r & 3) {
  case 0: e = (u16)(r >> 16); break;
  case 1: e = (u16)((r >> 16) & 0x8000 | (0x3fff - 70 + ((r >> 8) & 127))); break;
  case 2: return x_of(DXE[(r >> 8) % ND], DXM[(r >> 8) % ND]);
  default: e = (u16)((r >> 16) & 0x8000 | ((r >> 4) & 0xf) | ((r >> 24) & 0x7) << 12 | ((r >> 8) & 0xff) << 4); break;
  }
  if ((e & 0x7fff) == 0) m &= 0x7fffffffffffffffull;
  else m |= 0x8000000000000000ull;
  return x_of(e, m);
}

int main(void) {
  for (int op = 0; op < 2; op++) {
    for (int i = 0; i < ND * ND * ND * ND + RANDOM; i++) {
      float a, b, c, d;
      if (i < ND * ND * ND * ND) {
        a = f_of(DF[i / (ND * ND * ND)]); b = f_of(DF[i / (ND * ND) % ND]); c = f_of(DF[i / ND % ND]); d = f_of(DF[i % ND]);
      } else {
        a = f_of(rf()); b = f_of(rf()); c = f_of(rf()); d = f_of(rf());
      }
      float _Complex z = op ? __divsc3(a, b, c, d) : __mulsc3(a, b, c, d);
      pf(__real__ z); pf(__imag__ z);
    }
  }
  for (int op = 0; op < 2; op++) {
    for (int i = 0; i < ND * ND * ND * ND + RANDOM; i++) {
      double a, b, c, d;
      if (i < ND * ND * ND * ND) {
        a = d_of(DD[i / (ND * ND * ND)]); b = d_of(DD[i / (ND * ND) % ND]); c = d_of(DD[i / ND % ND]); d = d_of(DD[i % ND]);
      } else {
        a = d_of(rd()); b = d_of(rd()); c = d_of(rd()); d = d_of(rd());
      }
      double _Complex z = op ? __divdc3(a, b, c, d) : __muldc3(a, b, c, d);
      pd(__real__ z); pd(__imag__ z);
    }
  }
#ifndef NO_XC3
  for (int op = 0; op < 2; op++) {
    for (int i = 0; i < ND * ND * ND * ND + RANDOM; i++) {
      long double a, b, c, d;
      if (i < ND * ND * ND * ND) {
        int p = i / (ND * ND * ND), q = i / (ND * ND) % ND, r = i / ND % ND, s = i % ND;
        a = x_of(DXE[p], DXM[p]); b = x_of(DXE[q], DXM[q]); c = x_of(DXE[r], DXM[r]); d = x_of(DXE[s], DXM[s]);
      } else {
        a = rx(); b = rx(); c = rx(); d = rx();
      }
      long double _Complex z = op ? __divxc3(a, b, c, d) : __mulxc3(a, b, c, d);
      px(__real__ z); px(__imag__ z);
    }
  }
#endif
  return 0;
}
