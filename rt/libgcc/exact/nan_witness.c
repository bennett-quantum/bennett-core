/* Small raw-bit witnesses from cplx_exact.c. The native link uses the installed GCC archive
 * unless diagnostic helper objects are explicitly supplied. No NaN normalization. */
#define main sweep_main
#include "cplx_exact.c"
#undef main

int main(void) {
  float _Complex f;
  double _Complex d;
  /* mulsc3/muldc3 #190: (0 + 0i) * (positive qNaN + negative payload qNaNi). */
  f = __mulsc3(0, 0, f_of(DF[9]), f_of(DF[10]));
  pf(__real__ f); pf(__imag__ f); out('\n');
  d = __muldc3(0, 0, d_of(DD[9]), d_of(DD[10]));
  pd(__real__ d); pd(__imag__ d); out('\n');
  /* divsc3 #190: the same operands, divided. The SSE backend commutes the real numerator's add. */
  f = __divsc3(0, 0, f_of(DF[9]), f_of(DF[10]));
  pf(__real__ f); pf(__imag__ f); out('\n');
  /* divdc3 #9: (0 + 0i) / (0 + positive qNaNi). 0/0 generates another NaN internally. */
  d = __divdc3(0, 0, 0, d_of(DD[9]));
  pd(__real__ d); pd(__imag__ d); out('\n');
  /* divdc3 #19604: (1 + positive qNaNi) / (0 + smallest subnormali). */
  d = __divdc3(1, d_of(DD[9]), 0, d_of(DD[4]));
  pd(__real__ d); pd(__imag__ d); out('\n');
  return 0;
}
