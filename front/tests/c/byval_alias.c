/* byval_alias.c: a struct passed by value is a private copy, at an address of its own. Native: 0. */
struct S {
  long a, b, c;
};

__attribute__((noinline)) int same(struct S s, struct S *p) { return &s == p; }

int main(void) {
  struct S s = {1, 2, 3};
  return same(s, &s);
}
