// Native comparisons observe equality and non-nullness only. Keep physical frames and sibling calls
// disabled in both compilations for higher levels. No addresses from a dead frame are dereferenced.
#include <setjmp.h>
#define NI __attribute__((noinline))
#define AI __attribute__((always_inline)) inline
volatile unsigned ra_sink;
volatile unsigned ra_two = 2;

NI void *ra0(void) { ra_sink++; return __builtin_return_address(0); }
NI void *ra2(void) { ra_sink++; return __builtin_return_address(2); }
NI void *ra_bridge2(void) { void *p = ra2(); ra_sink++; return p; }
NI void *ra_bridge1(void) { void *p = ra_bridge2(); ra_sink++; return p; }
void *(*volatile ra_ptr)(void) = ra_bridge1;

int ra_indirect_levels(int x) {
  void *p[2];
  for (unsigned i = 0; i < ra_two; i++) p[i] = ra_ptr();
  void *q = ra_ptr();
  ra_sink += (unsigned)x;
  return (p[0] != 0) | ((p[0] == p[1]) << 1) | ((p[0] != q) << 2);
}
NI void ra_rec(unsigned n, void **zero, void **two) {
  ra_sink++;
  if (n) { ra_rec(n-1,zero,two); ra_sink++; }
  zero[n] = __builtin_return_address(0);
  two[n] = __builtin_return_address(2);
}
NI int ra_rec_wrapper(unsigned n) {
  void *a[24], *b[24];
  ra_rec(n,a,b);
  int result = 1;
  for (unsigned i=0; i+3<n; i++) result &= a[i] == a[0] && b[i] == a[0];
  result |= (a[n] != a[0]) << 1;
  result |= (b[n-1] != a[0]) << 2;
  ra_sink++;
  return result;
}
int ra_recursive_levels(int x) { return ra_rec_wrapper((unsigned)x%16+5); }

static jmp_buf ra_env;
static void *saved_zero, *saved_one;
// Separate calls with volatile effects keep both observations across the jump in the optimized IR.
NI void *ra_up1(void) { ra_sink++; return __builtin_return_address(1); }
NI void *ra_up2(void) { ra_sink++; return __builtin_return_address(2); }
NI void ra_throw(unsigned n) {
  ra_sink++;
  if (n) ra_throw(n-1);
  longjmp(ra_env, 9);
}
NI int ra_jump_body(int x) {
  saved_zero = ra_up1();
  saved_one = ra_up2();
  if (!setjmp(ra_env)) ra_throw((unsigned)x%8);
  void *a = ra_up1(), *b = ra_up2();
  ra_sink++;
  return (saved_zero == a) | ((saved_one == b) << 1) | ((a != 0 && b != 0) << 2);
}
int ra_jumps(int x) { int r = ra_jump_body(x); ra_sink++; return r; }

static AI void *ra_inline(void) { ra_sink++; return __builtin_return_address(0); }
static void *volatile ra_inline_saved;
NI int ra_inline_body(int x) {
  void *a = __builtin_return_address(0);
  ra_inline_saved = a;
  void *b = ra_inline();
  ra_sink += (unsigned)x;
  return (ra_inline_saved == b) | ((a != 0) << 1);
}
int ra_inlining(int x) { int r = ra_inline_body(x); ra_sink++; return r; }

// A saved return address survives a stack allocation and a call through another site.
NI int ra_stack_body(int x) {
  void *a = __builtin_return_address(0);
  volatile unsigned char bytes[(unsigned)x%23+1];
  bytes[0] = (unsigned char)x;
  void *b = ra0();
  return (a == __builtin_return_address(0)) | ((b != a) << 1) | ((b != 0) << 2) | ((bytes[0] == (unsigned char)x) << 3);
}
int ra_stack(int x) { int r = ra_stack_body(x); ra_sink++; return r; }

#ifdef NATIVE
#include <stdio.h>
#define SHOW(f) for (int i=0;i<32;i++) printf(#f " %d %d\n",i,f(i));
int main(void) {
  SHOW(ra_indirect_levels) SHOW(ra_recursive_levels) SHOW(ra_jumps) SHOW(ra_inlining) SHOW(ra_stack)
  return 0;
}
#endif
