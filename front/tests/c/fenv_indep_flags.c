/* Independent floating-point environment test: the five ISO C exception flags (inexact, underflow at
 * the subnormal boundary, overflow, division by zero, invalid), tested and cleared one by one, across
 * float, double and long double, plus strict optimiser preservation (dead-result flag raising and
 * non-CSE across feclearexcept / fesetround). Compared verbatim against native clang 18 + glibc at
 * -O0 and -O2. */
#include <fenv.h>
#include <float.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>

#pragma STDC FENV_ACCESS ON

#define FIVE_FLAGS (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)

static const int modes[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
static const int single_flags[5] = {FE_INVALID, FE_DIVBYZERO, FE_OVERFLOW, FE_UNDERFLOW, FE_INEXACT};

static void show_flags(const char *tag) {
  fenv_t e;
  fegetenv(&e);
  printf("%s t=%02x s=%02x x=%02x", tag, fetestexcept(FIVE_FLAGS), e.__mxcsr & FIVE_FLAGS,
         e.__status_word & FIVE_FLAGS);
  for (int i = 0; i < 5; ++i) printf(" %d", fetestexcept(single_flags[i]) != 0);
  putchar('\n');
}

/* Dead-result strict operations: optimiser at -O2 must not eliminate flag-raising operations. */
static __attribute__((noinline)) void raise_by_dead_ops(double a, double b) {
  (void)(a / b);
}

static __attribute__((noinline)) double strict_div(double a, double b) { return a / b; }

int main(void) {
  static volatile float f0 = 0.0f, fn0 = -0.0f, f1 = 1.0f, f2 = 2.0f, f3 = 3.0f;
  static volatile float fmin_sub = 0x1p-149f, fmin_norm = 0x1p-126f, fmax_v = FLT_MAX;
  static volatile float finf = INFINITY, fninf = -INFINITY;
  static volatile float fqnan = NAN, fsnan = __builtin_nansf("");

  static volatile double d0 = 0.0, dn0 = -0.0, d1 = 1.0, d2 = 2.0, d3 = 3.0;
  static volatile double dmin_sub = 0x1p-1074, dmin_norm = 0x1p-1022, dmax_v = DBL_MAX;
  static volatile double dinf = INFINITY, dninf = -INFINITY;
  static volatile double dqnan = NAN, dsnan = __builtin_nans("");

  static volatile long double l0 = 0.0L, ln0 = -0.0L, l1 = 1.0L, l2 = 2.0L, l3 = 3.0L;
  static volatile long double lmin_sub = 0x1p-16445L, lmin_norm = 0x1p-16382L, lmax_v = LDBL_MAX;
  static volatile long double linf = (long double)INFINITY, lninf = -(long double)INFINITY;
  static volatile long double lqnan = (long double)NAN, lsnan = __builtin_nansl("");

  /* 1. Exact subnormals must raise NEITHER FE_UNDERFLOW NOR FE_INEXACT. */
  fesetenv(FE_DFL_ENV);
  volatile float vf;
  volatile double vd;
  volatile long double vl;
  feclearexcept(FE_ALL_EXCEPT); vf = fmin_sub + fmin_sub; show_flags("exact_sub_f_add");
  feclearexcept(FE_ALL_EXCEPT); vf = fmin_norm * 0.5f; show_flags("exact_sub_f_mul");
  feclearexcept(FE_ALL_EXCEPT); vf = fmin_norm / f2; show_flags("exact_sub_f_div");
  feclearexcept(FE_ALL_EXCEPT); vd = dmin_sub + dmin_sub; show_flags("exact_sub_d_add");
  feclearexcept(FE_ALL_EXCEPT); vd = dmin_norm * 0.5; show_flags("exact_sub_d_mul");
  feclearexcept(FE_ALL_EXCEPT); vd = dmin_norm / d2; show_flags("exact_sub_d_div");
  feclearexcept(FE_ALL_EXCEPT); vl = lmin_sub + lmin_sub; show_flags("exact_sub_l_add");
  feclearexcept(FE_ALL_EXCEPT); vl = lmin_norm * 0.5L; show_flags("exact_sub_l_mul");
  feclearexcept(FE_ALL_EXCEPT); vl = lmin_norm / l2; show_flags("exact_sub_l_div");
  (void)vf; (void)vd; (void)vl;

  /* 2. Inexact subnormal results must raise BOTH FE_UNDERFLOW and FE_INEXACT. */
  feclearexcept(FE_ALL_EXCEPT); vf = fmin_sub / f3; show_flags("inexact_sub_f");
  feclearexcept(FE_ALL_EXCEPT); vd = dmin_sub / d3; show_flags("inexact_sub_d");
  feclearexcept(FE_ALL_EXCEPT); vl = lmin_sub / l3; show_flags("inexact_sub_l");
  feclearexcept(FE_ALL_EXCEPT); vf = fmin_sub * fmin_sub; show_flags("uflow_zero_f");
  feclearexcept(FE_ALL_EXCEPT); vd = dmin_sub * dmin_sub; show_flags("uflow_zero_d");
  feclearexcept(FE_ALL_EXCEPT); vl = lmin_sub * lmin_sub; show_flags("uflow_zero_l");

  /* 3. Tininess after rounding at the subnormal/normal boundary across all 4 modes.
   * Exact value is MIN_NORMAL - min_sub/4 (for FMA) or MIN_NORMAL - min_sub/2 (for mul/narrow):
   * rounds to MIN_NORMAL (no underflow!) in nearest & upward (for +) / downward (for -),
   * and rounds to largest subnormal (underflow!) in the other modes. */
  static volatile float fb_a = 0x1.fffffep-126f, fb_b = 0.25f, fb_c = 0x1p-127f;
  static volatile double db_a = 0x1.fffffffffffffp-1022, db_b = 0.25, db_c = 0x1p-1023;
  static volatile long double lb_a = 0x1.fffffffffffffffep-16382L, lb_b = 0.25L, lb_c = 0x1p-16383L;
  static volatile double d_for_f_tininess = 0x1p-126 - 0x1p-150;
  static volatile long double l_for_d_tininess = 0x1p-1022L - 0x1p-1075L;
  for (int m = 0; m < 4; ++m) {
    fesetround(modes[m]);
    printf("tiny_mode %d\n", m);
    feclearexcept(FE_ALL_EXCEPT); vf = fmaf(fb_a, fb_b, fb_c); show_flags("tiny_fmaf_pos");
    feclearexcept(FE_ALL_EXCEPT); vf = fmaf(-fb_a, fb_b, -fb_c); show_flags("tiny_fmaf_neg");
    feclearexcept(FE_ALL_EXCEPT); vd = fma(db_a, db_b, db_c); show_flags("tiny_fma_pos");
    feclearexcept(FE_ALL_EXCEPT); vd = fma(-db_a, db_b, -db_c); show_flags("tiny_fma_neg");
    feclearexcept(FE_ALL_EXCEPT); vl = fmal(lb_a, lb_b, lb_c); show_flags("tiny_fmal_pos");
    feclearexcept(FE_ALL_EXCEPT); vl = fmal(-lb_a, lb_b, -lb_c); show_flags("tiny_fmal_neg");
    feclearexcept(FE_ALL_EXCEPT); vf = (float)d_for_f_tininess; show_flags("tiny_d2f_pos");
    feclearexcept(FE_ALL_EXCEPT); vf = (float)(-d_for_f_tininess); show_flags("tiny_d2f_neg");
    feclearexcept(FE_ALL_EXCEPT); vd = (double)l_for_d_tininess; show_flags("tiny_l2d_pos");
    feclearexcept(FE_ALL_EXCEPT); vd = (double)(-l_for_d_tininess); show_flags("tiny_l2d_neg");
    /* Product at subnormal boundary: (1 - 2^-24) * 2^-126 = 2^-126 - 2^-150. */
    static volatile float f_near1 = 1.0f - 0x1p-24f;
    static volatile double d_near1 = 1.0 - 0x1p-53;
    static volatile long double l_near1 = 1.0L - 0x1p-64L;
    feclearexcept(FE_ALL_EXCEPT); vf = f_near1 * fmin_norm; show_flags("tiny_mulf");
    feclearexcept(FE_ALL_EXCEPT); vd = d_near1 * dmin_norm; show_flags("tiny_muld");
    feclearexcept(FE_ALL_EXCEPT); vl = l_near1 * lmin_norm; show_flags("tiny_mull");
  }

  /* 4. Overflow boundary across all 4 modes. */
  static volatile float fhalf_ulp = 0x1p103f, fulp = 0x1p104f;
  static volatile double dhalf_ulp = 0x1p970, dulp = 0x1p971;
  static volatile long double lhalf_ulp = 0x1p16319L, lulp = 0x1p16320L;
  for (int m = 0; m < 4; ++m) {
    fesetround(modes[m]);
    printf("ovf_mode %d\n", m);
    feclearexcept(FE_ALL_EXCEPT); vf = fmax_v + fhalf_ulp; show_flags("ovf_f_half");
    feclearexcept(FE_ALL_EXCEPT); vf = fmax_v + fulp; show_flags("ovf_f_ulp");
    feclearexcept(FE_ALL_EXCEPT); vd = dmax_v + dhalf_ulp; show_flags("ovf_d_half");
    feclearexcept(FE_ALL_EXCEPT); vd = dmax_v + dulp; show_flags("ovf_d_ulp");
    feclearexcept(FE_ALL_EXCEPT); vl = lmax_v + lhalf_ulp; show_flags("ovf_l_half");
    feclearexcept(FE_ALL_EXCEPT); vl = lmax_v + lulp; show_flags("ovf_l_ulp");
  }

  /* 5. Division by zero (ONLY FE_DIVBYZERO, never inexact/overflow/underflow) and inf/0 (no flags). */
  fesetenv(FE_DFL_ENV);
  feclearexcept(FE_ALL_EXCEPT); vf = f1 / f0; show_flags("dz_f_pos");
  feclearexcept(FE_ALL_EXCEPT); vf = f1 / fn0; show_flags("dz_f_neg");
  feclearexcept(FE_ALL_EXCEPT); vf = fmin_sub / f0; show_flags("dz_f_sub");
  feclearexcept(FE_ALL_EXCEPT); vf = finf / f0; show_flags("dz_f_inf");
  feclearexcept(FE_ALL_EXCEPT); vd = d1 / d0; show_flags("dz_d_pos");
  feclearexcept(FE_ALL_EXCEPT); vd = d1 / dn0; show_flags("dz_d_neg");
  feclearexcept(FE_ALL_EXCEPT); vd = dmin_sub / d0; show_flags("dz_d_sub");
  feclearexcept(FE_ALL_EXCEPT); vd = dinf / d0; show_flags("dz_d_inf");
  feclearexcept(FE_ALL_EXCEPT); vl = l1 / l0; show_flags("dz_l_pos");
  feclearexcept(FE_ALL_EXCEPT); vl = l1 / ln0; show_flags("dz_l_neg");
  feclearexcept(FE_ALL_EXCEPT); vl = lmin_sub / l0; show_flags("dz_l_sub");
  feclearexcept(FE_ALL_EXCEPT); vl = linf / l0; show_flags("dz_l_inf");

  /* 6. Invalid operations across arithmetic, sqrt, comparisons (signalling vs quiet), and conversions. */
  feclearexcept(FE_ALL_EXCEPT); vf = f0 / f0; show_flags("inv_f_0d0");
  feclearexcept(FE_ALL_EXCEPT); vf = finf / finf; show_flags("inv_f_idi");
  feclearexcept(FE_ALL_EXCEPT); vf = f0 * finf; show_flags("inv_f_0mi");
  feclearexcept(FE_ALL_EXCEPT); vf = finf + fninf; show_flags("inv_f_imi");
  feclearexcept(FE_ALL_EXCEPT); vf = sqrtf(-f1); show_flags("inv_f_sqrt");
  feclearexcept(FE_ALL_EXCEPT); vf = sqrtf(fninf); show_flags("inv_f_sqrtinf");
  feclearexcept(FE_ALL_EXCEPT); vf = sqrtf(fn0); show_flags("ok_f_sqrtn0");
  feclearexcept(FE_ALL_EXCEPT); vf = fqnan + f1; show_flags("ok_f_qnan_add");
  feclearexcept(FE_ALL_EXCEPT); vf = fsnan + f1; show_flags("inv_f_snan_add");
  feclearexcept(FE_ALL_EXCEPT); volatile int cmp = fqnan < f1; (void)cmp; show_flags("inv_f_qnan_lt");
  feclearexcept(FE_ALL_EXCEPT); cmp = fqnan == f1; show_flags("ok_f_qnan_eq");
  feclearexcept(FE_ALL_EXCEPT); cmp = fsnan == f1; show_flags("inv_f_snan_eq");
  feclearexcept(FE_ALL_EXCEPT); cmp = isless(fqnan, f1); show_flags("ok_f_isless_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = isless(fsnan, f1); show_flags("inv_f_isless_snan");

  feclearexcept(FE_ALL_EXCEPT); vd = d0 / d0; show_flags("inv_d_0d0");
  feclearexcept(FE_ALL_EXCEPT); vd = dinf + dninf; show_flags("inv_d_imi");
  feclearexcept(FE_ALL_EXCEPT); vd = sqrt(-d1); show_flags("inv_d_sqrt");
  feclearexcept(FE_ALL_EXCEPT); cmp = dqnan < d1; show_flags("inv_d_qnan_lt");
  feclearexcept(FE_ALL_EXCEPT); cmp = dqnan == d1; show_flags("ok_d_qnan_eq");
  feclearexcept(FE_ALL_EXCEPT); cmp = dsnan == d1; show_flags("inv_d_snan_eq");
  feclearexcept(FE_ALL_EXCEPT); cmp = isgreater(dqnan, d1); show_flags("ok_d_isgreater_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = isgreater(dsnan, d1); show_flags("inv_d_isgreater_snan");

  feclearexcept(FE_ALL_EXCEPT); vl = l0 / l0; show_flags("inv_l_0d0");
  feclearexcept(FE_ALL_EXCEPT); vl = linf + lninf; show_flags("inv_l_imi");
  feclearexcept(FE_ALL_EXCEPT); vl = sqrtl(-l1); show_flags("inv_l_sqrt");
  feclearexcept(FE_ALL_EXCEPT); cmp = lqnan < l1; show_flags("inv_l_qnan_lt");
  feclearexcept(FE_ALL_EXCEPT); cmp = lqnan == l1; show_flags("ok_l_qnan_eq");
  feclearexcept(FE_ALL_EXCEPT); cmp = lsnan == l1; show_flags("inv_l_snan_eq");
  feclearexcept(FE_ALL_EXCEPT); cmp = isunordered(lqnan, l1); show_flags("ok_l_isunord_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = isunordered(lsnan, l1); show_flags("inv_l_isunord_snan");

  /* Every comparison operator tested individually on both sides (qNaN vs sNaN). */
  feclearexcept(FE_ALL_EXCEPT); cmp = d1 <= dqnan; show_flags("inv_d_le_qnan_r");
  feclearexcept(FE_ALL_EXCEPT); cmp = dqnan > d1; show_flags("inv_d_gt_qnan_l");
  feclearexcept(FE_ALL_EXCEPT); cmp = d1 >= dqnan; show_flags("inv_d_ge_qnan_r");
  feclearexcept(FE_ALL_EXCEPT); cmp = !(dqnan < d1); show_flags("inv_d_notlt_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = dqnan != d1; show_flags("ok_d_ne_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = dsnan != d1; show_flags("inv_d_ne_snan");
  feclearexcept(FE_ALL_EXCEPT); cmp = islessequal(d1, dqnan); show_flags("ok_d_isle_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = islessequal(d1, dsnan); show_flags("inv_d_isle_snan");
  feclearexcept(FE_ALL_EXCEPT); cmp = isgreaterequal(dqnan, d1); show_flags("ok_d_isge_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = isgreaterequal(dsnan, d1); show_flags("inv_d_isge_snan");
  feclearexcept(FE_ALL_EXCEPT); cmp = islessgreater(dqnan, d1); show_flags("ok_d_islg_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = islessgreater(dsnan, d1); show_flags("inv_d_islg_snan");
  feclearexcept(FE_ALL_EXCEPT); cmp = l1 <= lqnan; show_flags("inv_l_le_qnan_r");
  feclearexcept(FE_ALL_EXCEPT); cmp = lqnan >= l1; show_flags("inv_l_ge_qnan_l");
  feclearexcept(FE_ALL_EXCEPT); cmp = lqnan != l1; show_flags("ok_l_ne_qnan");
  feclearexcept(FE_ALL_EXCEPT); cmp = lsnan != l1; show_flags("inv_l_ne_snan");

  /* FMA with 0 * inf + NaN: invalid only for sNaN in float/double, always for long double (glibc). */
  feclearexcept(FE_ALL_EXCEPT); vf = fmaf(f0, finf, fqnan); show_flags("fmaf_0_inf_qnan");
  feclearexcept(FE_ALL_EXCEPT); vf = fmaf(f0, finf, fsnan); show_flags("fmaf_0_inf_snan");
  feclearexcept(FE_ALL_EXCEPT); vf = fmaf(finf, f0, fqnan); show_flags("fmaf_inf_0_qnan");
  feclearexcept(FE_ALL_EXCEPT); vd = fma(d0, dinf, dqnan); show_flags("fma_0_inf_qnan");
  feclearexcept(FE_ALL_EXCEPT); vd = fma(d0, dinf, dsnan); show_flags("fma_0_inf_snan");
  feclearexcept(FE_ALL_EXCEPT); vd = fma(dinf, d0, dqnan); show_flags("fma_inf_0_qnan");
  feclearexcept(FE_ALL_EXCEPT); vl = fmal(l0, linf, lqnan); show_flags("fmal_0_inf_qnan");
  feclearexcept(FE_ALL_EXCEPT); vl = fmal(l0, linf, lsnan); show_flags("fmal_0_inf_snan");

  /* Out-of-range integer conversions: check FE_INVALID only (FE_INEXACT unspecified beside it). */
  static volatile double d_big = 1e30;
  static volatile long double l_40000 = 40000.0L;
  feclearexcept(FE_ALL_EXCEPT); volatile int32_t i32 = (int32_t)d_big; (void)i32; feclearexcept(FE_INEXACT); show_flags("inv_d_to_i32");
  feclearexcept(FE_ALL_EXCEPT); i32 = (int32_t)dqnan; feclearexcept(FE_INEXACT); show_flags("inv_qnan_to_i32");
  feclearexcept(FE_ALL_EXCEPT); volatile int16_t i16 = (int16_t)l_40000; (void)i16; feclearexcept(FE_INEXACT); show_flags("inv_l_to_i16");

  /* 7. Testing and clearing all five flags ONE BY ONE in forward and reverse orders, raised by real
   * operations in SSE, then in x87, then via feraiseexcept. */
  feclearexcept(FE_ALL_EXCEPT);
  vd = d0 / d0;              /* SSE invalid */
  vd = d1 / d0;              /* SSE divbyzero */
  vd = dmax_v * d2;          /* SSE overflow + inexact */
  vd = dmin_sub / d3;        /* SSE underflow + inexact */
  show_flags("sse_all_raised");
  for (int i = 0; i < 5; ++i) {
    feclearexcept(single_flags[i]);
    printf("sse_clr_%d ", i);
    show_flags("after");
  }

  feclearexcept(FE_ALL_EXCEPT);
  vl = l0 / l0;              /* x87 invalid */
  vl = l1 / l0;              /* x87 divbyzero */
  vl = lmax_v * l2;          /* x87 overflow + inexact */
  vl = lmin_sub / l3;        /* x87 underflow + inexact */
  show_flags("x87_all_raised");
  for (int i = 4; i >= 0; --i) {
    feclearexcept(single_flags[i]);
    printf("x87_clr_%d ", i);
    show_flags("after");
  }

  /* Both units hold flags simultaneously; clearing one flag clears it in BOTH units while keeping
   * all other flags in each unit. */
  feclearexcept(FE_ALL_EXCEPT);
  vd = d0 / d0;              /* SSE invalid */
  vd = d1 / d3;              /* SSE inexact */
  vl = l0 / l0;              /* x87 invalid */
  vl = l1 / l0;              /* x87 divbyzero */
  show_flags("both_before_clear");
  feclearexcept(FE_INVALID);
  show_flags("both_cleared_inv");
  feclearexcept(FE_DIVBYZERO);
  show_flags("both_cleared_dz");
  feclearexcept(FE_INEXACT);
  show_flags("both_cleared_inx");

  /* Raising individual flags via feraiseexcept one by one and clearing each. */
  for (int i = 0; i < 5; ++i) {
    feclearexcept(FE_ALL_EXCEPT);
    feraiseexcept(single_flags[i]);
    printf("raise_%d ", i);
    show_flags("set");
    feclearexcept(single_flags[i]);
    printf("raise_%d ", i);
    show_flags("clr");
  }

  /* 8. Optimiser preservation at -O2: dead-result operations and non-CSE across feclearexcept. */
  fesetenv(FE_DFL_ENV);
  feclearexcept(FE_ALL_EXCEPT);
  raise_by_dead_ops(d1, d0);
  show_flags("dead_divbyzero");
  feclearexcept(FE_ALL_EXCEPT);
  raise_by_dead_ops(d0, d0);
  show_flags("dead_invalid");

  double x = d1, y = d3;
  feclearexcept(FE_ALL_EXCEPT);
  volatile double r1 = strict_div(x, y);
  show_flags("cse_first");
  feclearexcept(FE_ALL_EXCEPT);
  volatile double r2 = strict_div(x, y);
  show_flags("cse_second");
  printf("cse_eq %d\n", r1 == r2);

  fesetenv(FE_DFL_ENV);
  return 0;
}
