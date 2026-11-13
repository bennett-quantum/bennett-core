/* corpus.c: ordinary scalar C functions of one 8-bit input, compiled both natively and by bennett-front.
 * CORPUS lists them; each takes an unsigned char and returns an int. Nothing here knows about qubits. */

#define CORPUS(X)                                                                                            \
  X(square_plus_one)                                                                                         \
  X(collatz_steps)                                                                                           \
  X(bit_count)                                                                                               \
  X(gcd_with_60)                                                                                             \
  X(signed_divide)                                                                                           \
  X(unsigned_divide)                                                                                         \
  X(shifts)                                                                                                  \
  X(compare_chain)                                                                                           \
  X(switch_on_residue)                                                                                       \
  X(fibonacci_mod)                                                                                           \
  X(calls_helper)                                                                                            \
  X(recursive_split)                                                                                         \
  X(table_lookup)                                                                                            \
  X(clamp_and_abs)                                                                                           \
  X(local_array)

int square_plus_one(unsigned char x) { return x * x + 1; }

/* Steps of the Collatz map to reach 1; a loop whose trip count depends on the input. */
int collatz_steps(unsigned char x) {
  unsigned n = x;
  int steps = 0;
  while (n > 1) {
    n = n % 2 ? 3 * n + 1 : n / 2;
    steps++;
  }
  return steps;
}

int bit_count(unsigned char x) {
  int count = 0;
  for (unsigned v = x; v; v >>= 1) {
    count += v & 1;
  }
  return count;
}

int gcd_with_60(unsigned char x) {
  unsigned a = x, b = 60;
  while (b) {
    unsigned t = a % b;
    a = b;
    b = t;
  }
  return (int)a;
}

/* Signed division and remainder: divides by zero at x = 0. */
int signed_divide(unsigned char x) {
  signed char s = (signed char)x;
  return 1000 / s + 1000 % s;
}

/* Unsigned division and remainder: divides by zero at x = 0. */
int unsigned_divide(unsigned char x) { return 200u / x * 7 + 200u % x; }

int shifts(unsigned char x) {
  signed char s = (signed char)x;
  return (s >> 2) ^ (int)((unsigned)x << 3) ^ (s >> 7) ^ (int)(x >> 5);
}

int compare_chain(unsigned char x) {
  signed char s = (signed char)x;
  if (s < -50) return 1;
  if (s < 0) return 2;
  if (s == 0) return 3;
  if (s > 100) return 4;
  return 5 + (x > 40);
}

int switch_on_residue(unsigned char x) {
  switch (x % 7) {
  case 0: return 10;
  case 1: return x * 2;
  case 3: return -x;
  case 5: return x ^ 0x55;
  case 6: return x / 3;
  default: return 0;
  }
}

int fibonacci_mod(unsigned char x) {
  unsigned a = 0, b = 1;
  for (unsigned i = 0; i < x; i++) {
    unsigned t = (a + b) % 1000;
    a = b;
    b = t;
  }
  return (int)a;
}

__attribute__((noinline)) int helper(int v) { return v * 3 - (v >> 1); }

int calls_helper(unsigned char x) { return helper(x) + helper(x + 1) * 2; }

__attribute__((noinline)) int split(int n) { return n <= 1 ? 1 : split(n / 2) + split(n / 3) + 1; }

/* Non-tail recursion, which stays a call. */
int recursive_split(unsigned char x) { return split(x); }

static const int table[16] = {3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9, 7, 9, 3};

int table_lookup(unsigned char x) { return table[x & 15] * 16 + table[x >> 4]; }

int clamp_and_abs(unsigned char x) {
  int s = (signed char)x;
  int clamped = s < -20 ? -20 : s > 30 ? 30 : s;
  int magnitude = s < 0 ? -s : s;
  return clamped * 100 + magnitude;
}

int local_array(unsigned char x) {
  int a[8];
  for (int i = 0; i < 8; i++) {
    a[i] = (x >> i) & 1 ? i * i : x + i;
  }
  int sum = 0;
  for (int i = 0; i < 8; i++) {
    sum = sum * 3 + a[(i * 5 + x) & 7];
  }
  return sum;
}
