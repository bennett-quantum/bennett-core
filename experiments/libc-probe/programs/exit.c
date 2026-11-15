#include <stdio.h>
#include <stdlib.h>
static void bye(void) { fputs("atexit\n", stdout); }
int main(void) {
  atexit(bye);
  printf("buffered %d", 7);
  exit(5);
}
