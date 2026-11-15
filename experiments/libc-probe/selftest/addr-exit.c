/* Harness self-test: as addr-return.c, but ends by exit(0) (the exit path): must be classified wrong. */
#include <stdio.h>
#include <stdlib.h>
int main(void) {
  int x;
  printf("%d\n", (unsigned long)&x >> 40 != 0);
  exit(0);
}
