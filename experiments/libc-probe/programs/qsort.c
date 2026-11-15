#include <stdlib.h>
static int cmp(const void *a, const void *b) {
  int x = *(const int *)a, y = *(const int *)b;
  return (x > y) - (x < y);
}
int main(void) {
  int v[16] = {5, -3, 9, 0, 12, 7, 7, -8, 1, 100, 2, 3, -1, 4, 6, 11};
  qsort(v, 16, sizeof v[0], cmp);
  for (int i = 1; i < 16; i++)
    if (v[i - 1] > v[i]) return 1;
  return v[0] == -8 && v[15] == 100 ? 0 : 2;
}
