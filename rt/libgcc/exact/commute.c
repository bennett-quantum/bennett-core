/* Native check that the GCC archive's float/double NaN choices are operand orders of the production IR.
 * commute.py routes every float/double fadd/fmul of target/front/complex/<helper>.bc through cm_<op>_f<bits>,
 * which performs the operation with IR operand 0 as the SSE destination unless bit i of cm_mask swaps the
 * operands, and records in cm_both the instructions that saw two NaN operands. For each helper this program
 * generates cplx_exact.c's tuples (same order, same random stream), searches the orientations of the
 * instructions that ever see two NaNs for one under which the IR reproduces the archive on the two-NaN
 * tuples, then requires that single fixed order to reproduce the archive's raw bits on EVERY tuple. */
#define main sweep_main
#include "cplx_exact.c"
#undef main
int printf(const char *, ...);
extern u64 cm_mask, cm_both;
float _Complex cm___mulsc3(float, float, float, float);
float _Complex cm___divsc3(float, float, float, float);
double _Complex cm___muldc3(double, double, double, double);
double _Complex cm___divdc3(double, double, double, double);

#define NT (ND * ND * ND * ND + RANDOM)
typedef struct { u64 r, i; } bits;
static int same(bits a, bits b) { return a.r == b.r && a.i == b.i; }

/* One helper: T is the type, GEN the operand generator, DIRECTED the directed table, OF its decoder. */
#define CHECK(NAME, T, FMT_BYTES, NATIVE_FN, IR_FN, OF, TABLE, GEN)                                      \
  static T NAME##_in[NT][4];                                                                              \
  static bits NAME##_want[NT];                                                                            \
  static bits NAME##_ir(int t, u64 mask) {                                                                \
    cm_mask = mask;                                                                                       \
    T _Complex z = IR_FN(NAME##_in[t][0], NAME##_in[t][1], NAME##_in[t][2], NAME##_in[t][3]);             \
    T x = __real__ z, y = __imag__ z;                                                                     \
    bits o = {0, 0};                                                                                      \
    __builtin_memcpy(&o.r, &x, FMT_BYTES);                                                                \
    __builtin_memcpy(&o.i, &y, FMT_BYTES);                                                                \
    return o;                                                                                             \
  }                                                                                                       \
  static int NAME##_check(void) {                                                                         \
    static int two[NT];                                                                                   \
    int ntwo = 0;                                                                                         \
    long differ = 0;                                                                                      \
    u64 relevant = 0;                                                                                     \
    for (int t = 0; t < NT; t++) {                                                                        \
      for (int k = 0; k < 4; k++)                                                                         \
        NAME##_in[t][k] = t < ND * ND * ND * ND ? OF(TABLE[t / (ND * ND * ND) * (k == 0) +                \
                                                         t / (ND * ND) % ND * (k == 1) +                  \
                                                         t / ND % ND * (k == 2) + t % ND * (k == 3)])     \
                                                : OF(GEN());                                              \
      T _Complex z = NATIVE_FN(NAME##_in[t][0], NAME##_in[t][1], NAME##_in[t][2], NAME##_in[t][3]);       \
      T x = __real__ z, y = __imag__ z;                                                                   \
      bits w = {0, 0};                                                                                    \
      __builtin_memcpy(&w.r, &x, FMT_BYTES);                                                              \
      __builtin_memcpy(&w.i, &y, FMT_BYTES);                                                              \
      NAME##_want[t] = w;                                                                                 \
      cm_both = 0;                                                                                        \
      differ += !same(NAME##_ir(t, 0), w);                                                                \
      NAME##_ir(t, ~0ull);                                                                                \
      if (cm_both) two[ntwo++] = t;                                                                       \
      relevant |= cm_both;                                                                                \
    }                                                                                                     \
    for (u64 m = relevant;; m = (m - 1) & relevant) {                                                     \
      int ok = 1;                                                                                         \
      for (int j = 0; j < ntwo && ok; j++) ok = same(NAME##_ir(two[j], m), NAME##_want[two[j]]);         \
      if (ok) {                                                                                           \
        long bad = 0;                                                                                     \
        for (int t = 0; t < NT; t++) bad += !same(NAME##_ir(t, m), NAME##_want[t]);                      \
        printf(#NAME ": %d tuples, %ld differ from the archive in IR operand order; swapping operands "    \
               "of instructions %llx (of %llx that see two NaNs) leaves %ld differing\n",                  \
               NT, differ, m, relevant, bad);                                                             \
        return bad != 0;                                                                                  \
      }                                                                                                   \
      if (!m) break;                                                                                      \
    }                                                                                                     \
    printf(#NAME ": %d tuples, %ld differ in IR operand order; NO fixed operand order reproduces the "      \
           "archive\n", NT, differ);                                                                      \
    return 1;                                                                                             \
  }

CHECK(mulsc3, float, 4, __mulsc3, cm___mulsc3, f_of, DF, rf)
CHECK(divsc3, float, 4, __divsc3, cm___divsc3, f_of, DF, rf)
CHECK(muldc3, double, 8, __muldc3, cm___muldc3, d_of, DD, rd)
CHECK(divdc3, double, 8, __divdc3, cm___divdc3, d_of, DD, rd)

int main(void) {
  /* The order of cplx_exact.c, so that the random tuples are the same. */
  int failed = mulsc3_check();
  failed |= divsc3_check();
  failed |= muldc3_check();
  failed |= divdc3_check();
  return failed;
}
