// __builtin_return_address. Each function `ra_*` takes one input and returns what C can observe of return
// addresses: equality between calls from one site, difference between sites, non-null. The test runs each
// on the device for many inputs and compares with this file compiled natively with -DNATIVE, whose main
// prints the same values. The raw addresses differ between the two machines and are never printed.
// Level 1 natively follows the frame-pointer chain, so the native build keeps frame pointers and makes no
// sibling calls; the device walks its own frames and has no tail calls.
#define NOINLINE __attribute__((noinline))

// Every helper writes this, so LLVM cannot treat two calls of it as one, and adds to it after its own
// calls, so none of them is a tail call.
volatile int sink;
// A loop bound LLVM cannot see, so no loop is unrolled into several call sites.
volatile int two = 2;

NOINLINE void *here(void) {
  sink++;
  return __builtin_return_address(0);
}

NOINLINE void *up(void) {
  sink++;
  return __builtin_return_address(1);
}

// The return address of a call of `up`'s caller.
NOINLINE void *middle(void) {
  void *r = up();
  sink++;
  return r;
}

// Calls of one site give one address, every time. The site is inside a function of its own, so however the
// optimiser peels or unrolls the loop, it stays one site.
NOINLINE void *one_site(void) {
  void *r = here();
  sink++;
  return r;
}
int ra_one_site(int x) {
  int n = x % 7 + 2;
  void *first = one_site();
  int same = 0;
  for (int i = 1; i < n; i++) {
    same += one_site() == first;
  }
  return same + 100 * (first != 0);
}

// Two sites, visited twice each.
int ra_two_sites(int x) {
  void *v[2][2];
  for (int i = 0, n = two; i < n; i++) {
    v[i][0] = here();
    if (x & 1) {
      sink++;
    }
    v[i][1] = here();
  }
  return (v[0][0] != v[0][1]) | (v[0][0] == v[1][0]) << 1 | (v[0][1] == v[1][1]) << 2;
}

NOINLINE void *site_a(void) {
  void *r = here();
  sink++;
  return r;
}
NOINLINE void *site_b(void) {
  void *r = here();
  sink++;
  return r;
}
NOINLINE void *site_c(void) {
  void *r = here();
  sink++;
  return r;
}

// Which of three sites a later call came from, by comparing with addresses recorded first.
int ra_which_site(int x) {
  void *seen[3] = {site_a(), site_b(), site_c()};
  void *r = x % 3 == 0 ? site_a() : x % 3 == 1 ? site_b() : site_c();
  int found = -1;
  for (int i = 0; i < 3; i++) {
    if (r == seen[i]) {
      found = found == -1 ? i : 10;
    }
  }
  return found;
}

// A recursion: every level but the outermost returns into `rec` itself, so to one site.
NOINLINE void rec(int n, void **out) {
  sink++;
  if (n > 0) {
    rec(n - 1, out);
    sink++;
  }
  out[n] = __builtin_return_address(0);
}
int ra_recursive(int x) {
  void *out[16];
  int n = x % 15 + 1;
  rec(n, out);
  int same = 0;
  for (int i = 1; i < n; i++) {
    same += out[i] == out[0];
  }
  return same | (out[n] != out[0]) << 8;
}

// Through a function pointer, from two sites.
void *(*volatile pointer)(void) = here;
int ra_through_a_pointer(int x) {
  void *a = pointer();
  void *b = (x & 1) ? pointer() : here();
  void *c = pointer();
  return (a != b) | (a != c) << 1 | (b != c) << 2;
}

// One level up: the caller of `middle`, which called from three sites, one of them twice.
int ra_level_one(int x) {
  void *a = middle();
  void *b = middle();
  void *c[2];
  for (int i = 0, n = two; i < n; i++) {
    c[i] = middle();
    sink += x;
  }
  return (a != b) | (b != c[0]) << 1 | (c[0] == c[1]) << 2 | (a != 0) << 3;
}

// The function's own return address: non-null.
int ra_own(int x) { return (__builtin_return_address(0) != 0) + x % 2; }

// As GCC's pr17377: a value returned through two frames keeps its return address constant.
NOINLINE void *f17377(int i) {
  if (sink++ == 0) {
    return __builtin_return_address(0);
  }
  switch (i) {
  case 1:
    return (void *)f17377;
  case 0:
    return __builtin_return_address(0);
  }
  return 0;
}
NOINLINE void *y17377(int i) {
  void *r = f17377(i);
  sink++;
  return r;
}
int ra_pr17377(int x) {
  sink = 0;
  void *v = y17377(4);
  return (y17377(1) == (void *)f17377) | (y17377(0) == v) << 1 | (y17377(x % 5 + 2) == 0) << 2;
}

#ifdef NATIVE
#include <stdio.h>
#define SHOW(f)                                                                                                     \
  for (int x = 0; x < 32; x++) {                                                                                     \
    printf(#f " %d %d\n", x, f(x));                                                                                  \
  }
int main(void) {
  SHOW(ra_one_site)
  SHOW(ra_two_sites)
  SHOW(ra_which_site)
  SHOW(ra_recursive)
  SHOW(ra_through_a_pointer)
  SHOW(ra_level_one)
  SHOW(ra_own)
  SHOW(ra_pr17377)
  return 0;
}
#endif
