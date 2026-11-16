/* byval_tail_call.c: a struct passed by value to a function that is not inlined, from a call clang marks
 * `tail`. Once the copy is passed as a plain pointer the call must lose that mark, or LLVM deletes the stores
 * that fill the copy (a `tail` call does not touch the caller's allocas). Native: 0. */
struct big { long a[8]; };

__attribute__((noinline)) static long sum(struct big b) {
  long s = 0;
  for (int i = 0; i < 8; i++) s = s * 3 + b.a[i];
  return s;
}

volatile int seed = 1;

int main(void) {
  struct big b;
  for (int i = 0; i < 8; i++) b.a[i] = seed * (i + 1);
  long x = sum(b), y = sum(b);
  b.a[3] = 9;
  long z = sum(b);
  if (x != y) return 1;
  if (z == x) return 2;
  return x == 4916 ? 0 : 3;
}
