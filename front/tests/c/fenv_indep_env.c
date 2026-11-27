/* Independent floating-point environment test: fegetenv / fesetenv / feholdexcept / feupdateenv /
 * fegetexceptflag / fesetexceptflag / fesetround / fegetround / FLT_ROUNDS round trips with flags
 * set in SSE, x87, or both, nested hold/update, and feupdateenv(FE_DFL_ENV). Compared verbatim
 * against native clang 18 + glibc at -O0 and -O2. */
#include <fenv.h>
#include <float.h>
#include <math.h>
#include <stdio.h>

#pragma STDC FENV_ACCESS ON

#define FIVE_FLAGS (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)

static volatile double d0 = 0.0, d1 = 1.0, d3 = 3.0, dmax = DBL_MAX, dtiny = 0x1p-1074;
static volatile long double l0 = 0.0L, l1 = 1.0L, l3 = 3.0L, lmax = LDBL_MAX, ltiny = 0x1p-16445L;

static void dump(const char *tag) {
  fenv_t e;
  fegetenv(&e);
  printf("%s r=%d flt=%d t=%02x s=%02x x=%02x sse_rc=%x x87_rc=%x\n", tag, fegetround(), FLT_ROUNDS,
         fetestexcept(FIVE_FLAGS), e.__mxcsr & FIVE_FLAGS, e.__status_word & FIVE_FLAGS,
         (e.__mxcsr >> 13) & 3u, (e.__control_word >> 10) & 3u);
}

static void probe_values(const char *tag) {
  double dv = d1 / d3;
  long double lv = l1 / l3;
  unsigned long long db = 0;
  unsigned char lb[10];
  __builtin_memcpy(&db, &dv, 8);
  __builtin_memcpy(lb, &lv, 10);
  printf("%s d=%016llx l=%02x%02x%02x%02x\n", tag, db, lb[9], lb[8], lb[1], lb[0]);
}

int main(void) {
  volatile double vd;
  volatile long double vl;

  /* 1. fegetenv / fesetenv round trips with non-default modes and flags in SSE, x87, and both. */
  fesetenv(FE_DFL_ENV);
  dump("init");

  fesetround(FE_UPWARD);
  vd = d1 / d0;      /* SSE divbyzero */
  vl = lmax * 2.0L;  /* x87 overflow + inexact */
  fenv_t saved1;
  printf("fegetenv1 %d\n", fegetenv(&saved1));
  dump("before_mutate1");

  /* Mutate everything: mode, clear old flags, raise different flags. */
  fesetround(FE_DOWNWARD);
  feclearexcept(FE_ALL_EXCEPT);
  vd = d0 / d0;      /* SSE invalid */
  vl = ltiny / l3;   /* x87 underflow + inexact */
  dump("mutated1");
  probe_values("mutated1_val");

  /* Restore saved1: mode AND exact per-unit flags must come back. */
  printf("fesetenv1 %d\n", fesetenv(&saved1));
  dump("restored1");
  probe_values("restored1_val");

  /* 2. feholdexcept / feupdateenv round trip with flags set before, cleared during, new flags
   * raised inside, and merged on update. */
  fesetenv(FE_DFL_ENV);
  fesetround(FE_TOWARDZERO);
  vd = d0 / d0;          /* SSE invalid */
  vl = ltiny / l3;       /* x87 underflow + inexact */
  dump("pre_hold");

  fenv_t held;
  printf("fehold %d\n", feholdexcept(&held));
  /* Mode preserved, all flags cleared in both units. */
  dump("in_hold_clear");
  probe_values("in_hold_val1");

  /* Change mode and raise FE_DIVBYZERO in SSE and FE_OVERFLOW in x87 inside the held region. */
  fesetround(FE_UPWARD);
  vd = d1 / d0;
  vl = lmax * 2.0L;
  dump("in_hold_raised");
  probe_values("in_hold_val2");

  /* feupdateenv restores FE_TOWARDZERO, restores held's flags, and merges currently raised flags. */
  printf("feupdate %d\n", feupdateenv(&held));
  dump("post_update");
  probe_values("post_update_val");

  /* 3. Nested feholdexcept / feupdateenv (two levels deep). */
  fesetenv(FE_DFL_ENV);
  fesetround(FE_DOWNWARD);
  feraiseexcept(FE_INVALID);
  fenv_t outer, inner;
  feholdexcept(&outer);
  dump("nest_outer_held");
  fesetround(FE_UPWARD);
  feraiseexcept(FE_DIVBYZERO);
  feholdexcept(&inner);
  dump("nest_inner_held");
  fesetround(FE_TOWARDZERO);
  feraiseexcept(FE_OVERFLOW);
  dump("nest_inner_raised");
  feupdateenv(&inner);
  dump("nest_after_inner");
  feupdateenv(&outer);
  dump("nest_after_outer");

  /* 4. feupdateenv(FE_DFL_ENV) with flags currently set: restores default mode, keeps current flags,
   * including when SSE holds overflow/underflow/inexact and x87 holds invalid/divbyzero (opposite to
   * feraiseexcept's units). */
  fesetenv(FE_DFL_ENV);
  fesetround(FE_UPWARD);
  vd = d1 / d0;      /* SSE divbyzero */
  vl = l1 / l3;      /* x87 inexact */
  dump("pre_update_dfl");
  printf("feupdate_dfl %d\n", feupdateenv(FE_DFL_ENV));
  dump("post_update_dfl");

  fesetenv(FE_DFL_ENV);
  fesetround(FE_DOWNWARD);
  vd = dmax * 2.0;   /* SSE overflow + inexact */
  vd = dtiny / d3;   /* SSE underflow + inexact */
  vl = l0 / l0;      /* x87 invalid */
  vl = l1 / l0;      /* x87 divbyzero */
  dump("pre_update_swap");
  printf("feupdate_swap %d\n", feupdateenv(FE_DFL_ENV));
  dump("post_update_swap");

  /* 5. fegetexceptflag / fesetexceptflag partial masks and per-unit clearing semantics. */
  fesetenv(FE_DFL_ENV);
  vd = d0 / d0;      /* SSE invalid */
  vd = d1 / d0;      /* SSE divbyzero */
  vl = lmax * 2.0L;  /* x87 overflow + inexact */
  fexcept_t part = 0, full = 0;
  printf("feget_part %d\n", fegetexceptflag(&part, FE_INVALID | FE_OVERFLOW));
  printf("feget_full %d\n", fegetexceptflag(&full, FE_ALL_EXCEPT));
  feclearexcept(FE_ALL_EXCEPT);
  feraiseexcept(FE_UNDERFLOW);
  dump("flag_only_uf");

  /* Restore only FE_INVALID from `part` (must leave FE_UNDERFLOW set and NOT set FE_OVERFLOW). */
  printf("feset_inv %d\n", fesetexceptflag(&part, FE_INVALID));
  dump("flag_after_inv");

  /* Restore FE_UNDERFLOW from `part` (where FE_UNDERFLOW is 0 -> must CLEAR FE_UNDERFLOW!). */
  printf("feset_clr_uf %d\n", fesetexceptflag(&part, FE_UNDERFLOW));
  dump("flag_after_clr_uf");

  /* Restore all five from `full`. */
  printf("feset_full %d\n", fesetexceptflag(&full, FE_ALL_EXCEPT));
  dump("flag_after_full");

  /* Per-unit fesetexceptflag: x87 has invalid+overflow, SSE clear; setting FE_INVALID only keeps x87
   * overflow and sets SSE invalid; clearing FE_OVERFLOW clears it in x87 too. */
  fesetenv(FE_DFL_ENV);
  vl = l0 / l0;
  vl = lmax * 2.0L;
  fexcept_t only_inv = FE_INVALID;
  printf("feset_unit_inv %d\n", fesetexceptflag(&only_inv, FE_INVALID));
  dump("flag_unit_inv");
  printf("feset_unit_clr_ovf %d\n", fesetexceptflag(&only_inv, FE_OVERFLOW));
  dump("flag_unit_clr_ovf");

  /* Verify fesetexceptflag with mask=0 changes nothing. */
  fexcept_t empty = 0;
  printf("feset_zero %d\n", fesetexceptflag(&empty, 0));
  dump("flag_after_zero");

  /* 6. fesetround invalid values must return nonzero and leave mode and flags untouched. */
  fesetenv(FE_DFL_ENV);
  fesetround(FE_DOWNWARD);
  feraiseexcept(FE_INVALID | FE_INEXACT);
  int bad_modes[] = {-1, 1, 0x100, 0x401, 0x1000, 0x2000};
  for (unsigned i = 0; i < sizeof(bad_modes) / sizeof(bad_modes[0]); ++i) {
    printf("bad_round %u nz=%d ", i, fesetround(bad_modes[i]) != 0);
    dump("kept");
  }

  /* 7. fesetenv(FE_DFL_ENV) resets everything. */
  printf("feset_dfl %d\n", fesetenv(FE_DFL_ENV));
  dump("final_dfl");
  (void)vd;
  (void)vl;
  return 0;
}
