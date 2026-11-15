#include <stdlib.h>
#include <string.h>
int main(void) {
  char *p[64];
  for (int i = 0; i < 64; i++) {
    p[i] = malloc(16 + 40 * i);
    if (!p[i]) return 1;
    memset(p[i], i, 16 + 40 * i);
  }
  for (int i = 0; i < 64; i += 2) free(p[i]);
  char *q = realloc(p[1], 5000);
  if (!q || q[0] != 1 || q[55] != 1) return 2;
  free(q);
  return 0;
}
