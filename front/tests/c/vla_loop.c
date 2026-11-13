/* vla_loop.c: variable-length arrays in loops. Each pass frees the arrays of the pass before (clang saves
 * the stack pointer before them and restores it after): 4000 passes of a 64-byte array need 256 KiB if
 * nothing is freed, more than the device stack the tests give. An inner array is freed while an outer one
 * is still in use: freeing too much would let the next inner array overwrite the outer one. Returns 0, or
 * the number of the first check that failed. */
volatile int eight = 8;

__attribute__((noinline)) long nested(int n, int m) {
  long total = 0;
  for (int i = 0; i < n; i++) {
    long outer[m];
    for (int k = 0; k < m; k++) outer[k] = i * 100 + k;
    for (int j = 0; j < 3; j++) {
      long inner[m + j];
      for (int k = 0; k < m + j; k++) inner[k] = -1;
      total += inner[j];
    }
    for (int k = 0; k < m; k++)
      if (outer[k] != i * 100 + k) return -1;
    total += outer[m - 1];
  }
  return total;
}

int main(void) {
  int m = eight;
  long sum = 0;
  for (int i = 0; i < 4000; i++) {
    long a[m];
    for (int k = 0; k < m; k++) a[k] = i + k;
    sum += a[m - 1];
  }
  if (sum != 4000L * 3999 / 2 + 4000L * 7) return 1;
  long n = nested(50, m);
  if (n != 50 * -3 + (100L * 49 * 50 / 2 + 50 * 7)) return 2;
  return 0;
}
