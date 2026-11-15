/* Harness self-test: whether a stack address is above 2^40 (native: yes; device: no, its memory is small).
 * Output differs, the run succeeds: must be classified wrong. */
#include <stdio.h>
int main(void) {
  int x;
  printf("%d\n", (unsigned long)&x >> 40 != 0);
  return 0;
}
