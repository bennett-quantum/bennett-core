// setjmp and longjmp in their C library and built-in forms. Each function `jump_*` takes one input and
// returns a value that depends on how its jumps went; the test runs each on the device for many inputs and
// compares with this file compiled natively with -DNATIVE, whose main prints the same values.
// Only what C defines is observed: locals changed between setjmp and longjmp are volatile.
#include <setjmp.h>
#include <stddef.h>

#define NOINLINE __attribute__((noinline))

// The value 0 becomes 1; any other value comes back as it is.
static jmp_buf basic_buf;
NOINLINE static void basic_throw(int v) { longjmp(basic_buf, v); }
int jump_value(int x) {
  int r = setjmp(basic_buf);
  if (r == 0) {
    basic_throw(x - 3); // x = 3 throws 0
  }
  return r;
}

// A jump across several frames of a recursion, each with an allocation, from the deepest.
static jmp_buf deep_buf;
NOINLINE static int deep(int n, int v) {
  volatile char pad[24];
  pad[n % 24] = (char)n;
  if (n == 0) {
    longjmp(deep_buf, v);
  }
  return deep(n - 1, v) + pad[n % 24];
}
int jump_across_frames(int x) {
  int r = setjmp(deep_buf);
  if (r == 0) {
    return deep(x % 12, x + 100);
  }
  return r;
}

// Repeated jumps to one setjmp: a volatile counter keeps its latest value.
static jmp_buf again_buf;
NOINLINE static void again_throw(int v) { longjmp(again_buf, v); }
int jump_repeatedly(int x) {
  volatile int count = 0;
  volatile int sum = 0;
  int r = setjmp(again_buf);
  count = count + 1;
  sum = sum + r;
  if (count <= x % 9) {
    again_throw(count * 3);
  }
  return sum * 16 + count;
}

// Nested setjmps: an inner one in a callee is jumped to, then the callee jumps out to the outer one
// (discarding the inner frame).
static jmp_buf outer_buf;
NOINLINE static int inner(int x) {
  jmp_buf inner_buf;
  volatile int hops = 0;
  int r = setjmp(inner_buf);
  hops = hops + 1;
  if (r < x % 5) {
    longjmp(inner_buf, r + 1);
  }
  longjmp(outer_buf, hops * 10 + r);
}
int jump_nested(int x) {
  int r = setjmp(outer_buf);
  if (r == 0) {
    inner(x);
  }
  return r;
}

// A jump from a callback, through a function that calls it by pointer.
static jmp_buf callback_buf;
NOINLINE static void callback(int v) {
  if (v > 2) {
    longjmp(callback_buf, v);
  }
}
NOINLINE static int each(void (*f)(int), int n) {
  int called = 0;
  for (int i = 0; i < n; i++) {
    f(i);
    called++;
  }
  return called;
}
int jump_from_callback(int x) {
  int r = setjmp(callback_buf);
  if (r == 0) {
    return -each(callback, x % 7);
  }
  return r;
}

// clang's built-in forms: the value is always 1.
static void *builtin_buf[5];
NOINLINE static void builtin_throw(void) { __builtin_longjmp(builtin_buf, 1); }
int jump_builtin(int x) {
  volatile int tries = 0;
  if (__builtin_setjmp(builtin_buf)) {
    if (tries < x % 4) {
      tries = tries + 1;
      builtin_throw();
    }
    return 100 + tries;
  }
  builtin_throw();
  return 0;
}

// Allocations made after setjmp are freed by the jump: a thousand jumps, each after allocating 1 KiB in
// a callee and in the setjmp frame, fit a 64 KiB stack only if the stack pointer comes back each time.
static jmp_buf alloca_buf;
NOINLINE static void alloca_throw(int n) {
  volatile char *p = __builtin_alloca(1024);
  p[n % 1024] = (char)n;
  longjmp(alloca_buf, n + 1);
}
int jump_frees_allocations(int x) {
  volatile char keep[16] = "kept";
  int r = setjmp(alloca_buf);
  if (r < 1000) {
    volatile char *q = __builtin_alloca(1024);
    q[0] = (char)r;
    alloca_throw(r);
  }
  return r + keep[x % 4] + x;
}

// A jmp_buf copied before the jump works like the original.
static jmp_buf copy_from, copy_to;
NOINLINE static void copy_throw(int v) { longjmp(copy_to, v); }
int jump_through_a_copy(int x) {
  int r = setjmp(copy_from);
  if (r == 0) {
    __builtin_memcpy(copy_to, copy_from, sizeof copy_from);
    copy_throw(x + 7);
  }
  return r * 2;
}

// The other names: _setjmp/_longjmp, sigsetjmp without saving the mask and siglongjmp.
static jmp_buf under_buf;
static sigjmp_buf sig_buf;
NOINLINE static void under_throw(int v) { _longjmp(under_buf, v); }
NOINLINE static void sig_throw(int v) { siglongjmp(sig_buf, v); }
int jump_other_names(int x) {
  int a = _setjmp(under_buf);
  if (a == 0) {
    under_throw(x + 1);
  }
  int b = sigsetjmp(sig_buf, 0);
  if (b == 0) {
    sig_throw(a * 3);
  }
  return a * 1000 + b;
}

// setjmp in a loop condition, re-armed each time round.
static jmp_buf loop_buf;
NOINLINE static void loop_throw(int v) { longjmp(loop_buf, v); }
int jump_in_a_loop(int x) {
  volatile int total = 0;
  for (volatile int i = 0; i < x % 6; i = i + 1) {
    int r = setjmp(loop_buf);
    if (r == 0) {
      loop_throw(i + 1);
    }
    total = total * 3 + r;
  }
  return total;
}

#define JUMPS(X)                                                                                                    \
  X(jump_value)                                                                                                     \
  X(jump_across_frames)                                                                                             \
  X(jump_repeatedly)                                                                                                \
  X(jump_nested)                                                                                                    \
  X(jump_from_callback)                                                                                             \
  X(jump_builtin)                                                                                                   \
  X(jump_frees_allocations)                                                                                         \
  X(jump_through_a_copy)                                                                                            \
  X(jump_other_names)                                                                                               \
  X(jump_in_a_loop)

#ifdef NATIVE
#include <stdio.h>
int main(void) {
#define PRINT(f)                                                                                                    \
  for (int x = 0; x < 32; x++)                                                                                      \
    printf("%s %d %d\n", #f, x, f(x));
  JUMPS(PRINT)
  return 0;
}
#endif
