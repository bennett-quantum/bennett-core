/* variadic_aligned.c: variadic arguments aligned to more than 16 bytes (GCC pr92904), and the 16-, 32- and
 * 64-byte vectors and long double / __int128 mixtures beside them, read where a native call puts them. clang's
 * va_arg rounds the address of such an argument up to its alignment, so the caller must place it there. Each
 * kind is passed after every number of ints and doubles that moves it from registers to the stack and shifts
 * the stack before it by 8 bytes, read directly, through va_copy and through a va_list passed on. Returns 0,
 * or the number of the first check that failed. */
#include <stdarg.h>

typedef int v4i __attribute__((vector_size(16)));
typedef double v4d __attribute__((vector_size(32)));
typedef float v16f __attribute__((vector_size(64)));

struct T { long a, b; } __attribute__((aligned(16)));
struct A { double a, b, c, d; } __attribute__((aligned(32)));
struct B { long a; } __attribute__((aligned(64)));
struct C { char c; long double x; } __attribute__((aligned(128)));
struct D { _Alignas(32) int x; __int128 y; long double z; };
struct E { short s; } __attribute__((aligned(256)));
struct F { v4d v; int i; };
struct G { long double x; __int128 y; char c; };
struct P { int x; } __attribute__((aligned(4096)));
typedef long along __attribute__((aligned(32)));
typedef __int128 a128 __attribute__((aligned(64)));

static long fold(long h, long v) { return h * 1000003 + v; }
static long ld(long double x) { return (long)(x * 8); }
static long i128(__int128 x) { return (long)(x >> 64) * 7 + (long)x; }

static long valT(struct T t) { return t.a * 3 + t.b; }
static long valA(struct A a) { return (long)(a.a + 2 * a.b + 3 * a.c + 5 * a.d); }
static long valB(struct B b) { return b.a; }
static long valC(struct C c) { return c.c * 100 + ld(c.x); }
static long valD(struct D d) { return d.x * 11 + i128(d.y) + ld(d.z); }
static long valE(struct E e) { return e.s; }
static long valF(struct F f) { return (long)(f.v[0] + 2 * f.v[1] + 3 * f.v[2] + 4 * f.v[3]) * 10 + f.i; }
static long valG(struct G g) { return ld(g.x) + i128(g.y) * 3 + g.c; }
static long valP(struct P p) { return p.x; }
static long vala(along a) { return a; }
static long valb(a128 b) { return i128(b); }
static long valv(v4i v) { return v[0] + 2 * v[1] + 3 * v[2] + 4 * v[3]; }
static long valw(v4d v) { return (long)(v[0] + 2 * v[1] + 3 * v[2] + 4 * v[3]); }
static long valx(v16f v) {
  long s = 0;
  for (int i = 0; i < 16; i++) s = s * 3 + (long)v[i];
  return s;
}

/* Kinds: 'i' int, 'd' double, 'L' long double, 'I' __int128, 'T'..'G' and 'P' the structs, 'a' along,
 * 'b' a128, 'v' v4i, 'w' v4d, 'x' v16f. */
__attribute__((noinline)) long vread(const char *kinds, va_list ap) {
  long h = 7;
  for (const char *k = kinds; *k; k++) {
    long v;
    switch (*k) {
    case 'i': v = va_arg(ap, int); break;
    case 'd': v = (long)(va_arg(ap, double) * 4); break;
    case 'L': v = ld(va_arg(ap, long double)); break;
    case 'I': v = i128(va_arg(ap, __int128)); break;
    case 'T': v = valT(va_arg(ap, struct T)); break;
    case 'A': v = valA(va_arg(ap, struct A)); break;
    case 'B': v = valB(va_arg(ap, struct B)); break;
    case 'C': v = valC(va_arg(ap, struct C)); break;
    case 'D': v = valD(va_arg(ap, struct D)); break;
    case 'E': v = valE(va_arg(ap, struct E)); break;
    case 'F': v = valF(va_arg(ap, struct F)); break;
    case 'G': v = valG(va_arg(ap, struct G)); break;
    case 'P': v = valP(va_arg(ap, struct P)); break;
    case 'a': v = vala(va_arg(ap, along)); break;
    case 'b': v = valb(va_arg(ap, a128)); break;
    case 'v': v = valv(va_arg(ap, v4i)); break;
    case 'w': v = valw(va_arg(ap, v4d)); break;
    case 'x': v = valx(va_arg(ap, v16f)); break;
    default: return -1;
    }
    h = fold(h, v);
  }
  return h;
}

/* Reads the arguments directly, then again through a copy passed on; both must agree. */
__attribute__((noinline)) long readall(const char *kinds, ...) {
  va_list ap, again;
  va_start(ap, kinds);
  va_copy(again, ap);
  long first = vread(kinds, ap);
  long second = vread(kinds, again);
  va_end(again);
  va_end(ap);
  return first == second ? first : -2;
}

/* A named parameter aligned to 64 bytes before the variadic ones: it takes its stack place first. */
__attribute__((noinline)) long named(struct B b, int n, ...) {
  va_list ap;
  va_start(ap, n);
  long h = fold(valB(b), n);
  struct A a = va_arg(ap, struct A);
  h = fold(h, valA(a));
  struct B c = va_arg(ap, struct B);
  h = fold(h, valB(c));
  va_end(ap);
  return h;
}

volatile int seed = 1;

static long hash(int n, const long *v) {
  long h = 7;
  for (int i = 0; i < n; i++) h = fold(h, v[i]);
  return h;
}

/* A struct aligned to 4096 bytes, each call in a frame of its own: an area and the copies cost about three
 * times the alignment, and the device stack is small. */
__attribute__((noinline)) int page1(void) {
  struct P p = {seed * 77};
  long e[] = {1, valP(p), 4};
  return readall("iPd", 1, p, 1.0) == hash(3, e);
}
__attribute__((noinline)) int page2(void) {
  struct P p = {seed * 77};
  long e[] = {1, 2, 3, 4, 5, 6, 7, valP(p), 8, valP(p)};
  return readall("iiiiiiiPiP", 1, 2, 3, 4, 5, 6, 7, p, 8, p) == hash(10, e);
}
__attribute__((noinline)) int page3(void) {
  struct P p = {seed * 77};
  long e[] = {4, 8, 12, 16, 20, 24, 28, 32, 36, valP(p), 1};
  return readall("dddddddddPi", 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, p, 1) == hash(11, e);
}


/* The values passed, set by main from `seed` so that nothing is folded. */
struct T t;
struct A a;
struct B b;
struct C c;
struct D d;
struct E e;
struct F f;
struct G g;
along al;
a128 bb;
v4i v;
v4d w;
v16f x;
long double l;
__int128 n;

/* Each kind after k ints and k doubles: k = 0 leaves all registers free, 6 and 8 fill them, and an odd k puts
 * an 8-byte argument on the stack first, so the aligned one must skip to its boundary. Each kind's checks are a
 * function of their own: every call has an area, and the device stack is small. Returns 0, or the number (1 to
 * 5) of the first check that failed. */
#define EACH(name, K, val, X)                                                                                  \
  __attribute__((noinline)) static int name(void) {                                                            \
    long e1[] = {1, val(X), 4};                                                                                \
    if (readall("i" K "d", 1, X, 1.0) != hash(3, e1)) return 1;                                                \
    long e2[] = {1, 2, 3, 4, 5, 6, 7, val(X), 8};                                                              \
    if (readall("iiiiiii" K "i", 1, 2, 3, 4, 5, 6, 7, X, 8) != hash(9, e2)) return 2;                          \
    long e3[] = {4, 8, 12, 16, 20, 24, 28, 32, 36, val(X), 1};                                                 \
    if (readall("ddddddddd" K "i", 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, X, 1) != hash(11, e3)) return 3; \
    long e4[] = {1, 2, 3, 4, 5, 6, 4, 8, 12, 16, 20, 24, 28, 32, val(X), ld(l), val(X), 9};                    \
    if (readall("iiiiiidddddddd" K "L" K "i", 1, 2, 3, 4, 5, 6, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, X, l, X, \
                9) != hash(18, e4))                                                                            \
      return 4;                                                                                                \
    long e5[] = {1, i128(n), 2, 3, 4, val(X), 12, i128(n), val(X)};                                            \
    if (readall("iIiii" K "dI" K, 1, n, 2, 3, 4, X, 3.0, n, X) != hash(9, e5)) return 5;                       \
    return 0;                                                                                                  \
  }

EACH(kindT, "T", valT, t)
EACH(kindA, "A", valA, a)
EACH(kindB, "B", valB, b)
EACH(kindC, "C", valC, c)
EACH(kindD, "D", valD, d)
EACH(kindE, "E", valE, e)
EACH(kindF, "F", valF, f)
EACH(kindG, "G", valG, g)
EACH(kinda, "a", vala, al)
EACH(kindb, "b", valb, bb)
EACH(kindv, "v", valv, v)
EACH(kindw, "w", valw, w)
EACH(kindx, "x", valx, x)

__attribute__((noinline)) static int all(void) {
  long e6[] = {valT(t), valA(a), valB(b), valC(c), valD(d), valE(e), valF(f), valG(g), valv(v), valw(w), valx(x)};
  return readall("TABCDEFGvwx", t, a, b, c, d, e, f, g, v, w, x) == hash(11, e6);
}

int main(void) {
  int s = seed;
  t = (struct T){s * 5, -s * 6};
  a = (struct A){s * 1.5, -s * 2.25, s * 3.0, s * 4.75};
  b = (struct B){s * -123456789L};
  c = (struct C){(char)(s * 9), s * 2.375L};
  d = (struct D){s * 17, ((__int128)(s * 3) << 64) | (unsigned)(s * 99), s * -6.5L};
  e = (struct E){(short)(s * 4321)};
  f = (struct F){{s * 1.0, s * 2.0, s * 3.0, s * 4.0}, s * 8};
  g = (struct G){s * 0.125L, ((__int128)(s * -2) << 64) | (unsigned)(s * 5), (char)(s * 3)};
  al = s * -31;
  bb = ((__int128)(s * 5) << 64) | (unsigned)(s * 21);
  v = (v4i){s, s * 2, s * -3, s * 4};
  w = (v4d){s * 0.5, s * 1.5, s * 2.5, s * 3.5};
  for (int i = 0; i < 16; i++) x[i] = (float)(s * i);
  l = s * 1.75L;
  n = ((__int128)(s * 11) << 64) | (unsigned)(s * 13);

  int (*const kinds[])(void) = {kindT, kindA, kindB, kindC, kindD, kindE, kindF,
                                kindG, kinda, kindb, kindv, kindw, kindx};
  for (int k = 0; k < (int)(sizeof kinds / sizeof *kinds); k++) {
    int failed = kinds[k]();
    if (failed) return 5 * k + failed; /* 1 to 65 */
  }
  if (!all()) return 70;
  if (named(b, 2, a, b) != fold(fold(fold(valB(b), 2), valA(a)), valB(b))) return 71;
  if (named(b, 2, a, b) != named(b, 2, a, b)) return 72;
  if (!page1()) return 73;
  if (!page2()) return 74;
  if (!page3()) return 75;
  return 0;
}
