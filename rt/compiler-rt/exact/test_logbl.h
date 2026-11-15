/* Test scaffolding only: musl's logbl (src/math/logbl.c) with ilogbl's result computed from the bits, without
 * the FENV_ACCESS pragma that the front refuses. Canonical encodings only. */
long double logbl(long double x) {
  unsigned long long m;
  unsigned short e;
  __builtin_memcpy(&m, &x, 8);
  __builtin_memcpy(&e, (char *)&x + 8, 2);
  int ex = e & 0x7fff;
  if (ex == 0x7fff) return x * x;
  if (ex == 0 && m == 0) return -1 / (x * x);
  if (ex == 0) return (long double)(63 - __builtin_clzll(m) - 16445);
  return (long double)(ex - 16383);
}
