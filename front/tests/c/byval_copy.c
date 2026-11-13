/* byval_copy.c: writes to a struct passed by value change the callee's copy only. Each callee reads its
 * copy through a function LLVM does not look into, so the stores happen; called twice on equal structs,
 * a callee sees the original values both times. Native: 17 + 2 + 3 * 1 + 2 * 1 = 24. */
struct S {
  long a, b, c;
};
struct V {
  volatile long a;
  long b, c;
};

__attribute__((noinline)) long first(const struct S *s) { return s->a; }
__attribute__((noinline)) long first_volatile(const struct V *v) { return v->a; }

__attribute__((noinline)) long change(struct S s) {
  s.a += 10;
  return first(&s) + s.b + s.c;
}

/* Returns the field's value on entry and overwrites it. */
__attribute__((noinline)) long swap(struct V v) {
  long old = v.a;
  v.a = 9;
  return old;
}

int main(void) {
  struct S s = {2, 2, 3};
  struct V v = {1, 2, 3}, w = {1, 2, 3};
  long r = change(s);
  long x = swap(v), y = swap(v), z = swap(w);
  return (int)(r + first(&s) + x + y + z + first_volatile(&v) + first_volatile(&w));
}
