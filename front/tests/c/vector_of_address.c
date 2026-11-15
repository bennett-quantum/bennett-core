/* vector_of_address.c: a one-element vector holding a function's address, returned by value (GCC's
 * pr71626). clang returns it as a double; once floating point is rewritten to integers, the optimiser
 * leaves a constant bitcast of a one-element vector, which must be the address it holds. Native: 0. */
typedef long V __attribute__((vector_size(sizeof(long))));

__attribute__((noinline)) V f(void) {
  V v = {(long)f};
  return v;
}

int main(void) {
  V v = f();
  return v[0] != (long)f;
}
