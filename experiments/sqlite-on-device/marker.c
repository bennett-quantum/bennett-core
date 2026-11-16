/* Demonstrate today's C front boundary, against the very same linked SQLite. */
#include "bennett.h"
int experiment_setup(void);
int experiment_query(int q, int r);
int main(void) {
  experiment_setup();
  unsigned row = 0;
  _Bool out = 0;
  bennett_h(bennett_bit(&row, 0));
  bennett_h(bennett_bit(&row, 1));
  out ^= experiment_query(2, row) != 0;
  return (int)bennett_measure(&out);
}
