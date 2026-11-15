#include <stdio.h>
volatile int v = 5;
static int x;
__attribute__((constructor)) static void init(void) { x = v; }
__attribute__((destructor)) static void fini(void) { puts("destructor"); }
int main(void) {
  printf("x=%d\n", x);
  return x;
}
