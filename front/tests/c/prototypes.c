/* Calls through a prototype that differs from the callee's definition, where the x86-64 System V convention
 * determines what each side reads: extra and missing (unread) arguments, integer width and signedness,
 * pointer and integer, variadic and fixed, floating-point and integer registers, stack slots, 128-bit
 * integers and two-register results. Each shape is called directly through a cast (mode 0), through a
 * volatile pointer of the other prototype (mode 1), and through an unprototyped declaration's type, directly
 * (mode 2) and through a volatile pointer (mode 3), where C can express it (shape 10 only through
 * pointers). Native -O0 and -O2 agree on every
 * word, which is the test that the convention, and not the compiler, decides it. */
typedef unsigned long U;
typedef unsigned __int128 W;
#define NI __attribute__((noinline))
#define SHAPES 21
#define MODES 4

/* Definitions. */
NI U c0(U a) { return a * 3 + 1; }
NI U c1(U a, U unused) { (void)unused; return a ^ 0x5555; }
NI long c2(long a) { return a * 0x10001 + 7; }
NI int c3(int a) { return a * 5 + 2; }
NI int c4(signed char a) { return a * 9; }
NI int c5(unsigned char a) { return a * 11; }
NI unsigned char c6(int a) { return (unsigned char)(a * 13 + 200); }
NI char *c7(char *p) { return p + 3; }
NI U c8(int n, ...) {
  __builtin_va_list list;
  __builtin_va_start(list, n);
  U sum = 0;
  for (int i = 0; i < n; i++) sum = sum * 31 + __builtin_va_arg(list, U);
  __builtin_va_end(list);
  return sum;
}
NI U c9(U a, U b) { return a * 7 - b; }
NI U c10(double d, U b) { return b * 3 + (U)d; }
NI float c11(float f) { return f * 2.0f + 1.0f; }
NI U c12(U a, U b, U c, U d, U e, U f, U g, U h) { return ((((((a * 3 + b) * 3 + c) * 3 + d) * 3 + e) * 3 + f) * 3 + g) * 3 + h; }
NI W c13(W a) { return a * 3 + (a >> 70); }
struct P { U a, b; };
NI struct P c14(U x) { struct P p = {x * 5, x ^ 9}; return p; }
NI void c15(U *p, U v) { *p = v * 17; }
NI U c16(void) { return 0x1234; }
NI _Bool c17(U x) { return (x & 6) != 0; }
NI U c18(_Bool b) { return b ? 77 : 99; }
NI short c19(short s) { return (short)(s * 3); }
NI U c20(U a, U b, U c, U d, U e, U f, U g) { return a + b + c + d + e + f + g * 1000; }

/* Callers. MODE 0: a cast of the function; 1: a volatile pointer of the other prototype; 2: a cast to the
 * unprototyped type; 3: a volatile unprototyped pointer. */
#define CALL(f, T, mode, ...) \
  ((mode) == 0   ? ((T)f)(__VA_ARGS__) \
   : (mode) == 1 ? ({ __typeof__(T) volatile p_ = (T)f; p_(__VA_ARGS__); }) \
                 : ((T)f)(__VA_ARGS__))
typedef U (*U0)();
typedef long (*L0)();
typedef int (*I0)();
typedef float (*F0)();
typedef W (*W0)();
typedef unsigned char (*B0)();
typedef signed char (*S0)();
static U0 volatile unprototyped_u;

NI U shape(unsigned k, U x, unsigned mode) {
  char buffer[16];
  switch (k) {
  case 0: /* extra arguments, registers */
    if (mode < 2) return CALL(c0, U (*)(U, U, U), mode, x, x + 1, x + 2);
    if (mode == 2) return ((U0)c0)(x, x + 1, x + 2);
    unprototyped_u = (U0)c0;
    return unprototyped_u(x, x + 1, x + 2);
  case 1: /* a missing argument the callee never reads */
    if (mode < 2) return CALL(c1, U (*)(U), mode, x);
    if (mode == 2) return ((U0)c1)(x);
    unprototyped_u = (U0)c1;
    return unprototyped_u(x);
  case 2: /* a long result read as int (c-testsuite 00025) */
    if (mode < 2) return (U)(long)CALL(c2, int (*)(long), mode, (long)x);
    if (mode == 2) return (U)(long)((I0)c2)((long)x);
    { I0 volatile p = (I0)c2; return (U)(long)p((long)x); }
  case 3: /* a long argument read as int */
    if (mode < 2) return (U)(long)CALL(c3, int (*)(long), mode, (long)x);
    if (mode == 2) return (U)(long)((I0)c3)((long)x);
    { I0 volatile p = (I0)c3; return (U)(long)p((long)x); }
  case 4: /* an int argument read as signed char: its extension, as the caller made it */
    if (mode < 2) return (U)(long)CALL(c4, int (*)(int), mode, (int)(signed char)x);
    if (mode == 2) return (U)(long)((I0)c4)((int)(signed char)x);
    { I0 volatile p = (I0)c4; return (U)(long)p((int)(signed char)x); }
  case 5: /* an int argument read as unsigned char */
    if (mode < 2) return (U)(long)CALL(c5, int (*)(int), mode, (int)(unsigned char)x);
    if (mode == 2) return (U)(long)((I0)c5)((int)(unsigned char)x);
    { I0 volatile p = (I0)c5; return (U)(long)p((int)(unsigned char)x); }
  case 6: /* an unsigned char result read as signed char */
    if (mode < 2) return (U)(long)CALL(c6, signed char (*)(int), mode, (int)x);
    if (mode == 2) return (U)(long)((S0)c6)((int)x);
    { S0 volatile p = (S0)c6; return (U)(long)p((int)x); }
  case 7: /* pointer and integer, both ways */
    if (mode < 2) return CALL(c7, U (*)(U), mode, (U)&buffer[x & 7]) - (U)buffer;
    if (mode == 2) return ((U0)c7)((U)&buffer[x & 7]) - (U)buffer;
    unprototyped_u = (U0)c7;
    return unprototyped_u((U)&buffer[x & 7]) - (U)buffer;
  case 8: /* a variadic function through a fixed prototype (integer arguments) */
    if (mode < 2) return CALL(c8, U (*)(int, U, U, U), mode, 3, x, x >> 3, ~x);
    if (mode == 2) return ((U0)c8)(3, x, x >> 3, ~x);
    unprototyped_u = (U0)c8;
    return unprototyped_u(3, x, x >> 3, ~x);
  case 9: /* a fixed function through a variadic prototype */
    if (mode < 2) return CALL(c9, U (*)(U, ...), mode, x, x + 3);
    if (mode == 2) return ((U0)c9)(x, x + 3);
    unprototyped_u = (U0)c9;
    return unprototyped_u(x, x + 3);
  case 10: /* integer and floating-point arguments in the other order: each in its own registers. Only
             * through pointers: clang -O2 passes a direct call's arguments as casts (the front refuses that) */
    if (mode < 2) return CALL(c10, U (*)(U, double), 1, x, (double)(x & 0xffff));
    unprototyped_u = (U0)c10;
    return unprototyped_u(x, (double)(x & 0xffff));
  case 11: { /* a float function with an extra argument; the unprototyped call passes a double */
    float r;
    if (mode < 2) r = CALL(c11, float (*)(float, U), mode, (float)(x & 0xfff), x);
    else if (mode == 2) r = ((F0)c11)((float)(x & 0xfff), x);
    else { F0 volatile p = (F0)c11; r = p((float)(x & 0xfff), x); }
    union { float f; unsigned u; } bits = {r};
    return bits.u;
  }
  case 12: /* eight parameters, two on the stack, and a ninth argument after them */
    if (mode < 2) return CALL(c12, U (*)(U, U, U, U, U, U, U, U, U), mode, x, (U)1, (U)2, (U)3, (U)4, (U)5, x >> 7, x >> 13, x >> 21);
    if (mode == 2) return ((U0)c12)(x, (U)1, (U)2, (U)3, (U)4, (U)5, x >> 7, x >> 13, x >> 21);
    unprototyped_u = (U0)c12;
    return unprototyped_u(x, (U)1, (U)2, (U)3, (U)4, (U)5, x >> 7, x >> 13, x >> 21);
  case 13: /* a 128-bit parameter from two 64-bit arguments, its result read as 64 bits */
    if (mode < 2) return CALL(c13, U (*)(U, U), mode, x, ~x);
    if (mode == 2) return ((U0)c13)(x, ~x);
    unprototyped_u = (U0)c13;
    return unprototyped_u(x, ~x);
  case 14: /* a two-register result read as its first register */
    if (mode < 2) return CALL(c14, U (*)(U), mode, x);
    if (mode == 2) return ((U0)c14)(x);
    unprototyped_u = (U0)c14;
    return unprototyped_u(x);
  case 15: { /* a void function, its "result" unused */
    U cell = 0;
    if (mode == 0) ((U (*)(U *, U))c15)(&cell, x);
    else if (mode == 1) { U (*volatile p)(U *, U) = (U (*)(U *, U))c15; p(&cell, x); }
    else if (mode == 2) ((U0)c15)(&cell, x);
    else { unprototyped_u = (U0)c15; unprototyped_u(&cell, x); }
    return cell;
  }
  case 16: /* no parameters, an argument given */
    if (mode < 2) return CALL(c16, U (*)(U), mode, x);
    if (mode == 2) return ((U0)c16)(x);
    unprototyped_u = (U0)c16;
    return unprototyped_u(x);
  case 17: /* a _Bool result read as unsigned char */
    if (mode < 2) return CALL(c17, unsigned char (*)(U), mode, x);
    if (mode == 2) return ((B0)c17)(x);
    { B0 volatile p = (B0)c17; return p(x); }
  case 18: /* an int argument read as _Bool */
    if (mode < 2) return CALL(c18, U (*)(int), mode, (int)(x & 1));
    if (mode == 2) return ((U0)c18)((int)(x & 1));
    unprototyped_u = (U0)c18;
    return unprototyped_u((int)(x & 1));
  case 19: /* a short result read as unsigned short, from an int argument */
    if (mode < 2) return CALL(c19, unsigned short (*)(int), mode, (int)(short)x);
    { unsigned short (*volatile p)() = (unsigned short (*)())c19; return mode == 2 ? ((unsigned short (*)())c19)((int)(short)x) : p((int)(short)x); }
  case 20: /* seven parameters, the last on the stack */
    if (mode < 2) return CALL(c20, U (*)(U, U, U, U, U, U, U), mode, x, (U)1, (U)2, (U)3, (U)4, (U)5, x >> 9);
    if (mode == 2) return ((U0)c20)(x, (U)1, (U)2, (U)3, (U)4, (U)5, x >> 9);
    unprototyped_u = (U0)c20;
    return unprototyped_u(x, (U)1, (U)2, (U)3, (U)4, (U)5, x >> 9);
  default:
    return 0;
  }
}

U result(unsigned k, U x, unsigned mode) { return shape(k, x, mode); }

#ifdef NATIVE
#include <stdio.h>
int main(void) {
  const U inputs[] = {0, 1, 7, 0x7fffffffffffffffUL, 0x8000000000000000UL, ~0UL, 0x0123456789abcdefUL, 0xfedcba9876543210UL};
  for (unsigned k = 0; k < SHAPES; k++)
    for (unsigned m = 0; m < MODES; m++)
      for (unsigned i = 0; i < 8; i++) printf("%lx\n", result(k, inputs[i], m));
}
#endif
