/* qubit_controlled_store.c: an ordinary store under the control of a qubit. Natively (with markers that
 * act classically) the store happens; outlining the branch once lost it, and the program returned 0. An
 * irreversible write under quantum control is not a unitary, so it is refused. */
#include "bennett.h"

volatile int sink;

int main(void) {
  _Bool q = 0;
  bennett_x(&q);
  if (q) sink = 7;
  bennett_measure(&q);
  return sink;
}
