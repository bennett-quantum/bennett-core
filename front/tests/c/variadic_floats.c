/* variadic_floats.c: structs of floats passed to a variadic function. clang passes `struct { float a[3]; }`
 * as a `<2 x float>` and a `float`, and `struct { float x, y; }` as one `<2 x float>`, each in a vector
 * register while one is free and on the stack after; the callee's va_arg reads them there (GCC's pr44575).
 * Mixed with doubles and integers, and with more than eight vector-register arguments. Returns 0, or the
 * number of the first check that failed. */
#include <stdarg.h>

struct three { float a[3]; };
struct two { float x, y; };

/* Kinds: 't' three, 'w' two, 'd' double, 'i' int. Each value is folded into an order-sensitive sum. */
__attribute__((noinline)) double fold(const char *kinds, ...) {
  va_list ap;
  va_start(ap, kinds);
  double sum = 0;
  for (const char *k = kinds; *k; k++) {
    sum *= 3;
    if (*k == 't') {
      struct three t = va_arg(ap, struct three);
      sum += t.a[0] + 10 * t.a[1] + 100 * t.a[2];
    } else if (*k == 'w') {
      struct two w = va_arg(ap, struct two);
      sum += w.x - 7 * w.y;
    } else if (*k == 'd') {
      sum += va_arg(ap, double);
    } else {
      sum += va_arg(ap, int);
    }
  }
  va_end(ap);
  return sum;
}

static double expect(const char *kinds, const double *values) {
  double sum = 0;
  for (const char *k = kinds; *k; k++) sum = sum * 3 + *values++;
  return sum;
}

int main(void) {
  struct three t = {{1.5f, -2.25f, 49026.0f}};
  struct two w = {0.75f, -3.5f};
  double vt = 1.5 + 10 * -2.25 + 100 * 49026.0, vw = 0.75 - 7 * -3.5;
  double v1[] = {vt, vt};
  if (fold("tt", t, t) != expect("tt", v1)) return 1;
  double v2[] = {vw, 2.5, vt, 4, vw};
  if (fold("wdtiw", w, 2.5, t, 4, w) != expect("wdtiw", v2)) return 2;
  /* Nine eightbytes of vector class: the last ones go to the stack. */
  double v3[] = {vt, vt, vt, vw, 0.125, vt, 9};
  if (fold("tttwdti", t, t, t, w, 0.125, t, 9) != expect("tttwdti", v3)) return 3;
  return 0;
}
