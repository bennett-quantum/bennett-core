/* fault_cancel.c: compute t ^= q, mark t with Z, uncompute, interfere q. With enough region budget the
 * phase kicks back onto q and q measures 1 on every run. With a budget of one step both applications
 * fault on every branch; with one fault register shared between them the two codes cancelled and the run
 * returned 0 as a success. Each application now has a fault register of its own. */
#include "bennett.h"

int main(void) {
  _Bool q = 0, t = 0;
  bennett_h(&q);
  t ^= q;
  bennett_z(&t);
  t ^= q;
  bennett_h(&q);
  return (int)bennett_measure(&q);
}
