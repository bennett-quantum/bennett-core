/* variadic.c: variadic functions and calls, checked against values computed without them. Returns 0, or
 * the number of the first check that failed. The cases are chosen to catch a wrong place for an argument:
 * registers against the stack (more than six integer arguments), a 16-byte-aligned struct that clang passes
 * as two plain integers when registers are free and in memory when they are not, structs with padding
 * copied by value, narrow and negative integers (sign extension), va_copy, a va_list passed on, a call
 * through a pointer, and a K&R call of a function that is not variadic. */
#include <stdarg.h>

struct small { char c; int i; };                         /* one register, with padding */
struct pair { long a, b; };                              /* two registers */
struct aligned { long a, b; } __attribute__((aligned(16))); /* two registers, or 16-byte-aligned memory */
struct three { int x, y, z; };                           /* {i64, i32} */
struct bytes { char b[3]; };                             /* three bytes */
struct big { char a; long b; char c; };                  /* 24 bytes, padded: always memory */
struct odd { char c[17]; };                              /* memory, a size that is not a multiple of 8 */

__attribute__((noinline)) long ints(int n, ...) {
  va_list ap;
  va_start(ap, n);
  long sum = 0;
  for (int i = 0; i < n; i++) sum = sum * 31 + va_arg(ap, int);
  va_end(ap);
  return sum;
}

/* Kinds: 'i' int, 'u' unsigned, 'l' long, 'p' pointer to long, 's' small, 'q' pair, 'a' aligned, 't' three,
 * 'b' bytes, 'g' big, 'o' odd. Each value is folded into an order-sensitive hash. */
__attribute__((noinline)) long mixed(const char *kinds, ...) {
  va_list ap;
  va_start(ap, kinds);
  long h = 7;
  for (const char *k = kinds; *k; k++) {
    long v = 0;
    switch (*k) {
    case 'i': v = va_arg(ap, int); break;
    case 'u': v = va_arg(ap, unsigned); break;
    case 'l': v = va_arg(ap, long); break;
    case 'p': v = *va_arg(ap, long *); break;
    case 's': { struct small s = va_arg(ap, struct small); v = s.c * 1000 + s.i; break; }
    case 'q': { struct pair p = va_arg(ap, struct pair); v = p.a * 1000 + p.b; break; }
    case 'a': { struct aligned a = va_arg(ap, struct aligned); v = a.a * 1000 + a.b; break; }
    case 't': { struct three t = va_arg(ap, struct three); v = (t.x * 1000 + t.y) * 1000 + t.z; break; }
    case 'b': { struct bytes b = va_arg(ap, struct bytes); v = (b.b[0] * 256 + b.b[1]) * 256 + b.b[2]; break; }
    case 'g': { struct big g = va_arg(ap, struct big); v = (g.a * 1000 + g.b) * 1000 + g.c; break; }
    case 'o': { struct odd o = va_arg(ap, struct odd); v = o.c[0] * 1000 + o.c[16]; break; }
    default: return -1;
    }
    h = h * 1000003 + v;
  }
  va_end(ap);
  return h;
}

/* The same hash, of values given in an array, for the expected results. */
static long hash(int n, const long *v) {
  long h = 7;
  for (int i = 0; i < n; i++) h = h * 1000003 + v[i];
  return h;
}

__attribute__((noinline)) long vsum(int n, va_list ap) {
  long s = 0;
  for (int i = 0; i < n; i++) s = s * 10 + va_arg(ap, long);
  return s;
}

/* Reads the arguments twice through va_copy, and once more through a va_list passed on. */
__attribute__((noinline)) long twice(int n, ...) {
  va_list ap, again;
  va_start(ap, n);
  va_copy(again, ap);
  long first = 0;
  for (int i = 0; i < n; i++) first = first * 10 + va_arg(ap, long);
  long second = vsum(n, again);
  va_end(again);
  va_end(ap);
  va_start(ap, n);
  long third = vsum(n, ap);
  va_end(ap);
  return first == second && second == third ? first : -1;
}

/* Recursion through a variadic function: each level takes one argument off and passes the rest. */
__attribute__((noinline)) long countdown(int n, ...) {
  if (n == 0) return 0;
  va_list ap;
  va_start(ap, n);
  long a = va_arg(ap, long), b = va_arg(ap, long), c = va_arg(ap, long);
  va_end(ap);
  return a + 2 * countdown(n - 1, b, c, a);
}

/* Named parameters that do not fit in registers take stack slots before the variadic arguments: eight
 * integers, and a pair that clang passes in memory because only one register is left for it. */
__attribute__((noinline)) long many(long a, long b, long c, long d, long e, long f, long g, long h, ...) {
  va_list ap;
  va_start(ap, h);
  long x = va_arg(ap, long), y = va_arg(ap, int);
  struct aligned z = va_arg(ap, struct aligned);
  va_end(ap);
  return ((((a + b + c + d + e + f) * 100 + g) * 100 + h) * 100 + x) * 100 + y + z.a * 7 + z.b;
}

__attribute__((noinline)) long split(int a, int b, int c, int d, int e, struct pair p, int g, ...) {
  va_list ap;
  va_start(ap, g);
  int h = va_arg(ap, int);
  long i = va_arg(ap, long);
  va_end(ap);
  return (((p.a * 10 + p.b) * 10 + g) * 10 + h) * 10 + i + a + b + c + d + e;
}

int kr();
int kr(a, b) int a; long b; { return a * 100 + (int)b; }

long (*volatile through)(int, ...) = ints;

int main(void) {
  if (ints(3, 1, -2, 3) != (1 * 31 - 2) * 31 + 3) return 1;
  if (ints(9, 1, 2, 3, 4, 5, 6, 7, 8, -9) != ((((((((1L * 31 + 2) * 31 + 3) * 31 + 4) * 31 + 5) * 31 + 6) * 31 + 7) *
                                                31 + 8) * 31 - 9))
    return 2;
  signed char sc = -5;
  short sh = -300;
  unsigned char uc = 250;
  if (ints(3, sc, sh, uc) != (-5L * 31 - 300) * 31 + 250) return 3;

  long cell = -123456789012L;
  struct small s = {-3, 70000};
  struct pair q = {11, -22};
  struct aligned a = {33, 44};
  struct three t = {1, -2, 3};
  struct bytes b = {{5, -6, 7}};
  struct big g = {-1, 9, 2};
  {
    long v[] = {-1, 4000000000L, -5000000000L, cell, -3 * 1000 + 70000, 11 * 1000 - 22, 33 * 1000 + 44};
    if (mixed("iulpsqa", -1, 4000000000U, -5000000000L, &cell, s, q, a) != hash(7, v)) return 4;
  }
  {
    /* One integer before the aligned struct: in registers it starts at an odd register, in memory at a
     * 16-byte boundary. */
    long v[] = {5, 33 * 1000 + 44, 6, 33 * 1000 + 44, 11 * 1000 - 22};
    if (mixed("iaiaq", 5, a, 6, a, q) != hash(5, v)) return 5;
  }
  {
    /* Registers run out in the middle of a pair, of an aligned struct, and before the rest. */
    long v[] = {1, 2, 3, 4, 11 * 1000 - 22, 33 * 1000 + 44, 7, 33 * 1000 + 44, (1 * 1000 - 2) * 1000 + 3,
                (5 * 256 - 6) * 256 + 7, (-1 * 1000 + 9) * 1000 + 2, -3 * 1000 + 70000, 8};
    if (mixed("iiiiqaiatbgsi", 1, 2, 3, 4, q, a, 7, a, t, b, g, s, 8) != hash(13, v)) return 6;
  }
  {
    long v[] = {(-1 * 1000 + 9) * 1000 + 2, (-1 * 1000 + 9) * 1000 + 2, 1, (1 * 1000 - 2) * 1000 + 3, 2};
    if (mixed("ggitl", g, g, 1, t, 2L) != hash(5, v)) return 7;
  }
  {
    /* All registers taken: an integer on the stack, then the aligned struct at the next 16-byte boundary;
     * a 17-byte struct, then integers after its rounded-up slot. */
    struct odd o = {{-4}};
    o.c[16] = 9;
    long v[] = {1, 2, 3, 4, 5, 6, 33 * 1000 + 44, -4 * 1000 + 9, 7, 33 * 1000 + 44, -4 * 1000 + 9, 8};
    if (mixed("iiiiiiaoiaoi", 1, 2, 3, 4, 5, 6, a, o, 7, a, o, 8) != hash(12, v)) return 15;
  }
  if (twice(4, 1L, 2L, 3L, 4L) != 1234) return 8;
  if (twice(8, 1L, 2L, 3L, 4L, 5L, 6L, 7L, 8L) != 12345678) return 9;
  if (countdown(3, 1L, 2L, 3L) != 1 + 2 * (2 + 2 * (3 + 2 * 0))) return 10;
  if (through(2, 7, -8) != 7 * 31 - 8) return 11;
  long total = 0;
  for (int i = 0; i < 100; i++) total += ints(2, i, -i);
  if (total != 30 * 4950) return 12;
  if (kr(3, 4L) != 304) return 13;
  if (many(1, 1, 1, 1, 1, 1, 2, 3, 4L, -5, a) != (((6 * 100 + 2) * 100 + 3) * 100 + 4) * 100 - 5 + 33 * 7 + 44)
    return 16;
  struct pair p2 = {1, 2};
  if (split(0, 0, 0, 0, 1, p2, 3, 4, 5L) != (((1 * 10 + 2) * 10 + 3) * 10 + 4) * 10 + 5 + 1) return 17;
  /* The struct passed by value is a copy: changing it after the call changes nothing. */
  struct big copy = g;
  long before = mixed("g", copy);
  copy.b = 1000;
  if (mixed("g", g) != before) return 14;
  return 0;
}
