/* <fenv.h> itself against the native program: the rounding mode set, read and rejected, the flags raised,
 * tested, cleared, saved and restored, whole environments saved, held, set and updated, FLT_ROUNDS, and the
 * flags of a few operations between them. Prints one line per check; `front/tests/tests/fenv.rs` compares the
 * output with the native program's. */
#include <fenv.h>
#include <float.h>
#include <math.h>
#include <stdio.h>

#pragma STDC FENV_ACCESS ON

#define FLAGS (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)

static volatile double one = 1.0, three = 3.0, zero = 0.0, huge = 1e300, tiny = 1e-300;
static volatile float onef = 1.0f, threef = 3.0f;
static volatile long double onel = 1.0L, threel = 3.0L;

static void show(const char *what, double x) {
  union {
    double d;
    unsigned long long u;
  } bits = {x};
  printf("%s %016llx %02x %d %d\n", what, bits.u, fetestexcept(FLAGS), fegetround(), FLT_ROUNDS);
}

/* A strict function that is not inlined, called twice with the same operands across a change of mode: the
 * optimiser must not merge the calls (the environment is the runtime's memory, not LLVM's inaccessible one). */
static __attribute__((noinline)) double add(double x, double y) { return x + y; }
static double (*volatile add_through)(double, double) = add;
static volatile long double below_two63 = 0x1.fffffffffffffffep62L;

static void third(const char *what) {
  long double l = onel / threel;
  unsigned char bytes[10];
  __builtin_memcpy(bytes, &l, 10);
  printf("%s %a %a %02x%02x%02x ", what, (double)(onef / threef), one / three, bytes[2], bytes[1], bytes[0]);
  show("", -one / three);
}

int main(void) {
  /* Modes. */
  int modes[] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
  for (int i = 0; i < 4; ++i) {
    printf("set %d -> %d\n", modes[i], fesetround(modes[i]));
    third("third");
  }
  printf("bad mode %d, still %d\n", fesetround(0x400 | 1) != 0, fegetround());
  printf("bad mode %d, still %d\n", fesetround(-1) != 0, fegetround());
  fesetround(FE_TONEAREST);

  /* Flags. */
  feclearexcept(FE_ALL_EXCEPT);
  show("clear", 0);
  show("inexact", one / three);
  show("overflow", huge * huge);
  feclearexcept(FE_OVERFLOW);
  show("cleared overflow", 0);
  show("underflow", tiny * tiny);
  show("divide", one / zero);
  show("invalid", zero / zero);
  feclearexcept(FE_ALL_EXCEPT);
  feraiseexcept(FE_OVERFLOW | FE_INVALID);
  show("raised", 0);
  printf("test %x %x %x\n", fetestexcept(FE_OVERFLOW), fetestexcept(FE_INEXACT), fetestexcept(FE_ALL_EXCEPT));

  /* Saved flags. */
  fexcept_t saved;
  fegetexceptflag(&saved, FE_INVALID | FE_INEXACT);
  feclearexcept(FE_ALL_EXCEPT);
  show("cleared all", 0);
  fesetexceptflag(&saved, FE_ALL_EXCEPT);
  show("restored all", 0);
  feclearexcept(FE_ALL_EXCEPT);
  feraiseexcept(FE_DIVBYZERO);
  fesetexceptflag(&saved, FE_INVALID);
  show("restored invalid", 0);

  /* Environments. */
  fenv_t outer, held;
  fesetround(FE_UPWARD);
  feclearexcept(FE_ALL_EXCEPT);
  feraiseexcept(FE_UNDERFLOW);
  fegetenv(&outer);
  fesetround(FE_DOWNWARD);
  feraiseexcept(FE_DIVBYZERO);
  show("changed", one / three);
  fesetenv(&outer);
  show("restored env", 0);
  fesetenv(FE_DFL_ENV);
  show("default env", 0);
  fesetround(FE_TOWARDZERO);
  feraiseexcept(FE_INEXACT);
  printf("hold %d\n", feholdexcept(&held));
  show("held", 0);
  fesetround(FE_UPWARD);
  show("in hold", one / three);
  feraiseexcept(FE_OVERFLOW);
  feupdateenv(&held);
  show("updated", 0);
  third("after update");

  /* Two calls of a strict function across a change of mode, and one through a pointer. */
  fesetenv(FE_DFL_ENV);
  fesetround(FE_DOWNWARD);
  volatile double down = add(one, 0x1p-53);
  int down_flags = fetestexcept(FLAGS);
  feclearexcept(FE_ALL_EXCEPT);
  fesetround(FE_UPWARD);
  volatile double up = add(one, 0x1p-53);
  int up_flags = fetestexcept(FLAGS);
  feclearexcept(FE_ALL_EXCEPT);
  volatile double through = add_through(one, 0x1p-53);
  printf("calls %d %x %x %d %x\n", down != up, down_flags, up_flags, up == through, fetestexcept(FLAGS));

  /* lrintl out of range after rounding upward: one conversion, invalid only. */
  feclearexcept(FE_ALL_EXCEPT);
  printf("lrintl %ld %x\n", lrintl(below_two63), fetestexcept(FLAGS));
  fesetround(FE_TONEAREST);

  /* A quiet comparison with a signalling NaN constant still raises invalid. */
  feclearexcept(FE_ALL_EXCEPT);
  volatile int constant = __builtin_nans("") == 0.0;
  printf("constant compare %d %x\n", constant, fetestexcept(FLAGS));

  /* Operations whose flags depend on the mode. */
  fesetenv(FE_DFL_ENV);
  for (int i = 0; i < 4; ++i) {
    fesetround(modes[i]);
    feclearexcept(FE_ALL_EXCEPT);
    show("max+max", DBL_MAX + DBL_MAX * (one / 4));
    feclearexcept(FE_ALL_EXCEPT);
    show("rint 2.5", rint(2.5 * one));
    feclearexcept(FE_ALL_EXCEPT);
    show("nearbyint -2.5", nearbyint(-2.5 * one));
    feclearexcept(FE_ALL_EXCEPT);
    show("lrint -2.5", (double)lrint(-2.5 * one));
    feclearexcept(FE_ALL_EXCEPT);
    show("x - x", three - three);
    feclearexcept(FE_ALL_EXCEPT);
    show("int", (double)(int)(three / 2));
  }
  fesetenv(FE_DFL_ENV);
  return 0;
}
