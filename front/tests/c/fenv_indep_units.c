/* Independent floating-point environment test: x87 (long double) and SSE (float/double) rounding
 * modes and exception flags set apart from each other across all 16 mode pairs, with interleaved
 * float/double/long double arithmetic, sqrt, fma, rint/nearbyint/lrint, format conversions, integer
 * conversions, fegetenv/fesetenv, feholdexcept/feupdateenv, and fesetround resynchronisation.
 * Compared verbatim against native clang 18 + glibc at -O0 and -O2. */
#include <fenv.h>
#include <float.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>

#pragma STDC FENV_ACCESS ON

#define FIVE_FLAGS (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)

static const int modes[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};

static void set_split(int sse_mode, int x87_mode) {
  fenv_t e;
  fesetenv(FE_DFL_ENV);
  fegetenv(&e);
  e.__mxcsr = (e.__mxcsr & ~0x6000u) | ((unsigned)sse_mode << 3);
  e.__control_word = (unsigned short)((e.__control_word & ~0x0c00u) | (unsigned)x87_mode);
  fesetenv(&e);
}

static void show_halves(const char *tag) {
  fenv_t e;
  fegetenv(&e);
  printf("%s r=%d flt=%d t=%02x s=%02x x=%02x src=%x xrc=%x\n", tag, fegetround(), FLT_ROUNDS,
         fetestexcept(FIVE_FLAGS), e.__mxcsr & FIVE_FLAGS, e.__status_word & FIVE_FLAGS,
         (e.__mxcsr >> 13) & 3u, (e.__control_word >> 10) & 3u);
}

static void print_hex(const void *p, size_t n) {
  for (size_t k = n; k-- > 0;) printf("%02x", ((const unsigned char *)p)[k]);
}

int main(void) {
  static volatile float f1 = 1.0f, f3 = 3.0f, fm1 = -1.0f, fhalf = 0.5f;
  static volatile double d1 = 1.0, d3 = 3.0, dm1 = -1.0, dhalf = 0.5, d0 = 0.0, dmax = DBL_MAX;
  static volatile long double l1 = 1.0L, l3 = 3.0L, lm1 = -1.0L, lhalf = 0.5L, l0 = 0.0L, lmax = LDBL_MAX;
  static volatile double d_tie = 1.0 + 0x1p-24;
  static volatile long double l_tie = 1.0L + 0x1p-53L;
  static volatile int64_t i64_inexact = (1LL << 53) + 1;
  static volatile __int128 i128_inexact = ((__int128)1 << 64) + 1;

  /* 1. All 16 (SSE mode, x87 mode) pairs: verify interleaved float, double, and long double ops
   * each follow their own unit's rounding mode and raise flags only in their own unit. */
  for (int sm = 0; sm < 4; ++sm) {
    for (int xm = 0; xm < 4; ++xm) {
      set_split(modes[sm], modes[xm]);
      printf("pair s%d x%d r=%d flt=%d\n", sm, xm, fegetround(), FLT_ROUNDS);

      /* SSE double op -> flags in SSE only. */
      feclearexcept(FE_ALL_EXCEPT);
      double dq = d1 / d3;
      double dnq = dm1 / d3;
      double dsq = sqrt(d3);
      double dfm = fma(1.0 + 0x1p-52, 1.0 + 0x1p-52, -d1);
      double dr = rint(dhalf);
      double dnr = nearbyint(-dhalf);
      long dlr = lrint(dhalf);
      float df_narrow = (float)d_tie;
      double di = (double)i64_inexact;
      printf("  sse ");
      print_hex(&dq, 8); putchar(' ');
      print_hex(&dnq, 8); putchar(' ');
      print_hex(&dsq, 8); putchar(' ');
      print_hex(&dfm, 8); putchar(' ');
      print_hex(&dr, 8); putchar(' ');
      print_hex(&dnr, 8); putchar(' ');
      printf("%ld ", dlr);
      print_hex(&df_narrow, 4); putchar(' ');
      print_hex(&di, 8); putchar(' ');
      show_halves("f");

      /* SSE float op -> flags in SSE only. */
      feclearexcept(FE_ALL_EXCEPT);
      float fq = f1 / f3;
      float fnq = fm1 / f3;
      float fr = rintf(fhalf);
      printf("  f32 ");
      print_hex(&fq, 4); putchar(' ');
      print_hex(&fnq, 4); putchar(' ');
      print_hex(&fr, 4); putchar(' ');
      show_halves("f");

      /* x87 long double op -> flags in x87 only (including long double -> double narrowing!). */
      feclearexcept(FE_ALL_EXCEPT);
      long double lq = l1 / l3;
      long double lnq = lm1 / l3;
      long double lsq = sqrtl(l3);
      long double lfm = fmal(1.0L + 0x1p-63L, 1.0L + 0x1p-63L, -l1);
      long double lr = rintl(lhalf);
      long double lnr = nearbyintl(-lhalf);
      long llr = lrintl(lhalf);
      double ld_narrow = (double)l_tie;
      long double li = (long double)i128_inexact;
      printf("  x87 ");
      print_hex(&lq, 10); putchar(' ');
      print_hex(&lnq, 10); putchar(' ');
      print_hex(&lsq, 10); putchar(' ');
      print_hex(&lfm, 10); putchar(' ');
      print_hex(&lr, 10); putchar(' ');
      print_hex(&lnr, 10); putchar(' ');
      printf("%ld ", llr);
      print_hex(&ld_narrow, 8); putchar(' ');
      print_hex(&li, 10); putchar(' ');
      show_halves("f");
    }
  }

  /* 2. Distinct exception flags in SSE vs x87 under split modes, plus feholdexcept/feupdateenv
   * preserving split modes, plus fesetround resynchronising both units. */
  set_split(FE_UPWARD, FE_DOWNWARD);
  volatile double dv = d1 / d0;        /* SSE divbyzero */
  volatile long double lv = lmax * 2.0L; /* x87 overflow + inexact */
  show_halves("split_flags_set");

  /* Clearing FE_DIVBYZERO affects SSE only; x87 keeps overflow+inexact. */
  feclearexcept(FE_DIVBYZERO);
  show_halves("split_cleared_dz");

  /* feholdexcept preserves split modes while clearing all flags. */
  fenv_t held;
  feholdexcept(&held);
  show_halves("split_held");

  /* Inside hold, invert the split modes and raise SSE invalid. */
  set_split(FE_TOWARDZERO, FE_UPWARD);
  dv = d0 / d0;
  show_halves("split_inner");

  /* feupdateenv restores (UPWARD, DOWNWARD) and merges flags. */
  feupdateenv(&held);
  show_halves("split_updated");

  /* fesetround synchronises BOTH units to FE_TOWARDZERO. */
  fesetround(FE_TOWARDZERO);
  show_halves("resync_tz");
  double resync_d = d1 / d3;
  long double resync_l = l1 / l3;
  printf("resync_vals ");
  print_hex(&resync_d, 8); putchar(' ');
  print_hex(&resync_l, 10); putchar('\n');

  /* Double-check exact cancellation sign under split modes: SSE=DOWNWARD gives -0.0, x87=UPWARD gives +0.0L. */
  set_split(FE_DOWNWARD, FE_UPWARD);
  double dc = d1 - d1;
  long double lc = l1 - l1;
  printf("cancel_split ");
  print_hex(&dc, 8); putchar(' ');
  print_hex(&lc, 10); putchar('\n');

  (void)dv;
  (void)lv;
  fesetenv(FE_DFL_ENV);
  return 0;
}
