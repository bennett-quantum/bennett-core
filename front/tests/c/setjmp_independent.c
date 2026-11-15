// Independent directed probes. Standard setjmp occurs only in C's permitted control expressions.
// No modified non-volatile automatic is read after a jump. Built-ins follow clang's separate rules.
#include <limits.h>
#include <setjmp.h>
#include <string.h>

#define NOINLINE __attribute__((noinline))
static jmp_buf a, b;
static volatile int stage, total;

NOINLINE static void throw_value(jmp_buf buf, int value) { longjmp(buf, value); }

int probe_values(int x) {
  const int values[] = {0, 1, -1, INT_MIN, INT_MAX, 256, -256, 0x12345678};
  switch (setjmp(a)) {
  case 0: throw_value(a, values[x % 8]);
  case 1: return 11;
  case -1: return 12;
  case INT_MIN: return 13;
  case INT_MAX: return 14;
  case 256: return 15;
  case -256: return 16;
  case 0x12345678: return 17;
  default: return 99;
  }
}

int probe_interleaved(int x) {
  stage = total = 0;
  if (setjmp(a)) {
    total += 3;
    if (stage == 2) {
      stage = 3;
      throw_value(b, x + 1);
    }
    return total;
  }
  if (setjmp(b)) {
    total += 5;
    if (stage == 1) {
      stage = 2;
      throw_value(a, x + 2);
    }
    stage = 4;
    throw_value(a, x + 3);
  }
  stage = 1;
  throw_value(b, x + 4);
  return 99;
}

struct saved {
  unsigned before;
  jmp_buf buf;
  unsigned after;
};

NOINLINE static void throw_copy(struct saved *s, int x) {
  volatile char pad[128];
  pad[x] = 7;
  throw_value(s->buf, pad[x]);
}

int probe_local_copy(int x) {
  struct saved original = {123, {0}, 456}, copy;
  volatile int local = x;
  if (!setjmp(original.buf)) {
    memcpy(&copy, &original, sizeof copy);
    memset(original.buf, 0xa5, sizeof original.buf);
    local += 17;
    throw_copy(&copy, x);
  }
  // copy was modified: observe only the volatile local and unchanged original canaries.
  return local + (int)original.before + (int)original.after;
}

NOINLINE static void recursive(int n, jmp_buf outer, int x) {
  struct saved s = {99, {0}, 77};
  volatile int count = 0;
  if (!setjmp(s.buf)) {
    if (n) recursive(n - 1, s.buf, x);
    else throw_value(s.buf, x + 1);
  }
  ++count;
  if (count < 3) throw_value(s.buf, x + count);
  total += (int)s.before + (int)s.after + count;
  throw_value(outer, x + 1);
}

int probe_recursive_records(int x) {
  total = 0;
  if (!setjmp(a)) recursive(x % 8, a, x);
  return total;
}

NOINLINE static void touch(volatile char *p, int x) { p[0] = (char)x; }

int probe_vla(int x) {
  volatile unsigned char kept[x + 17];
  kept[x] = (unsigned char)(x + 43);
  volatile int count = 0;
  if (!setjmp(a)) {
  again:
    ;
    volatile char scratch[x + 1024];
    touch(scratch, x);
    ++count;
    throw_value(a, 1);
  }
  if (count < 40) goto again;
  return count + kept[x];
}

static void *builtin[5], *builtin_copy[5];
NOINLINE static void builtin_throw(int n) {
  volatile int pad[16];
  pad[0] = n;
  if (n) builtin_throw(n - 1);
  else __builtin_longjmp(builtin_copy, 1);
  total += pad[0];
}

int probe_builtin_copy(int x) {
  volatile int local = x;
  if (__builtin_setjmp(builtin)) return local;
  memcpy(builtin_copy, builtin, sizeof builtin);
  local += 29;
  builtin_throw(x % 7);
  return 99;
}

int probe_loop_expression(int x) {
  volatile int n = 0;
  while (setjmp(a) == 0) {
    ++n;
    if (n == x + 1) throw_value(a, -9);
  }
  return n;
}

int probe_unchanged_locals(int x) {
  int unchanged = x * 31 + 17;
  volatile int changed = 0;
  if (!setjmp(a)) {
    changed = 77;
    throw_value(a, x + 1);
  }
  return unchanged + changed;
}

#define PROBES(X) \
  X(probe_values) \
  X(probe_interleaved) \
  X(probe_local_copy) \
  X(probe_recursive_records) \
  X(probe_vla) \
  X(probe_builtin_copy) \
  X(probe_loop_expression) \
  X(probe_unchanged_locals)

#ifdef NATIVE
#include <stdio.h>
int main(void) {
#define PRINT(f) for (int x = 0; x < 16; ++x) printf("%s %d %d\n", #f, x, f(x));
  PROBES(PRINT)
  return 0;
}
#endif
