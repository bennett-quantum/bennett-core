/* A tiny C runtime for running self-checking C test programs as Bennett device programs.
 *
 * The device has no C library and no exit path other than returning from the entry function or a named fault.
 * `abort` and `exit` therefore end the run in a named fault, chosen so that the fault kind tells them apart:
 *
 *   variant A (default):  abort, exit(non-zero) -> divide_by_zero      exit(0) -> bad_indirect
 *   variant B (-DRT_B):   abort, exit(non-zero) -> bad_indirect        exit(0) -> divide_by_zero
 *
 * A program whose device run ends in one of these two faults is run again with the other variant. If the fault
 * kind swaps, the runtime raised it; if it stays, the program itself faulted that way. The faults are forced
 * through volatile globals so that LLVM can neither fold the division nor the call away; if one somehow does
 * not fault, the loop around it runs into the step budget instead of returning.
 *
 * The whole runtime is linked under every test (llvm-link rt.bc --override test.bc), so a test's own definition
 * of one of these names wins, and so library calls that LLVM's optimiser introduces inside bennett-front (memcmp
 * to bcmp, malloc plus memset to calloc, ...) still find a definition.
 *
 * Memory and string routines are plain loops. `malloc` is a bump allocator over a static array and `free` does
 * nothing. Nothing here is variadic: `printf` and friends are deliberately left undefined, so a program that
 * calls them is refused by name ("a call to @printf, which has no definition").
 */
typedef __SIZE_TYPE__ size_t;

static volatile int rt_zero = 0;
static volatile int rt_one = 1;
volatile int rt_sink;
static void (*volatile rt_bad_target)(void) = (void (*)(void))(size_t)0x7e57;

static void __attribute__((noinline, noreturn)) rt_divide_by_zero(void) {
  for (;;) {
    rt_sink = rt_one / rt_zero;
  }
}

static void __attribute__((noinline, noreturn)) rt_bad_indirect(void) {
  for (;;) {
    rt_bad_target();
  }
}

#ifdef RT_B
#define RT_FAILURE rt_bad_indirect
#define RT_SUCCESS rt_divide_by_zero
#else
#define RT_FAILURE rt_divide_by_zero
#define RT_SUCCESS rt_bad_indirect
#endif

void abort(void) { RT_FAILURE(); }

void exit(int status) {
  if (status == 0) {
    RT_SUCCESS();
  }
  RT_FAILURE();
}

void _exit(int status) { exit(status); }

void *memcpy(void *dst, const void *src, size_t n) {
  unsigned char *d = dst;
  const unsigned char *s = src;
  for (size_t i = 0; i < n; i++) {
    d[i] = s[i];
  }
  return dst;
}

void *memmove(void *dst, const void *src, size_t n) {
  unsigned char *d = dst;
  const unsigned char *s = src;
  if (d < s) {
    for (size_t i = 0; i < n; i++) {
      d[i] = s[i];
    }
  } else {
    for (size_t i = n; i > 0; i--) {
      d[i - 1] = s[i - 1];
    }
  }
  return dst;
}

void *memset(void *dst, int c, size_t n) {
  unsigned char *d = dst;
  for (size_t i = 0; i < n; i++) {
    d[i] = (unsigned char)c;
  }
  return dst;
}

int memcmp(const void *a, const void *b, size_t n) {
  const unsigned char *x = a, *y = b;
  for (size_t i = 0; i < n; i++) {
    if (x[i] != y[i]) {
      return x[i] < y[i] ? -1 : 1;
    }
  }
  return 0;
}

int bcmp(const void *a, const void *b, size_t n) { return memcmp(a, b, n); }

void *memchr(const void *s, int c, size_t n) {
  const unsigned char *p = s;
  for (size_t i = 0; i < n; i++) {
    if (p[i] == (unsigned char)c) {
      return (void *)(p + i);
    }
  }
  return 0;
}

size_t strlen(const char *s) {
  size_t n = 0;
  while (s[n]) {
    n++;
  }
  return n;
}

int strcmp(const char *a, const char *b) {
  while (*a && *a == *b) {
    a++;
    b++;
  }
  return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
  for (; n > 0; n--, a++, b++) {
    if (*a != *b || !*a) {
      return (unsigned char)*a - (unsigned char)*b;
    }
  }
  return 0;
}

char *strcpy(char *dst, const char *src) {
  char *d = dst;
  while ((*d++ = *src++)) {
  }
  return dst;
}

char *stpcpy(char *dst, const char *src) {
  while ((*dst = *src++)) {
    dst++;
  }
  return dst;
}

char *strncpy(char *dst, const char *src, size_t n) {
  size_t i = 0;
  for (; i < n && src[i]; i++) {
    dst[i] = src[i];
  }
  for (; i < n; i++) {
    dst[i] = 0;
  }
  return dst;
}

char *strcat(char *dst, const char *src) {
  strcpy(dst + strlen(dst), src);
  return dst;
}

char *strchr(const char *s, int c) {
  for (;; s++) {
    if (*s == (char)c) {
      return (char *)s;
    }
    if (!*s) {
      return 0;
    }
  }
}

char *strrchr(const char *s, int c) {
  const char *found = 0;
  for (;; s++) {
    if (*s == (char)c) {
      found = s;
    }
    if (!*s) {
      return (char *)found;
    }
  }
}

int atoi(const char *s) {
  int sign = 1, value = 0;
  while (*s == ' ' || (*s >= '\t' && *s <= '\r')) {
    s++;
  }
  if (*s == '-' || *s == '+') {
    sign = *s++ == '-' ? -1 : 1;
  }
  while (*s >= '0' && *s <= '9') {
    value = value * 10 + (*s++ - '0');
  }
  return sign * value;
}

int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }
long long llabs(long long x) { return x < 0 ? -x : x; }

static unsigned char rt_heap[1 << 20] __attribute__((aligned(16)));
static size_t rt_heap_used;

void *malloc(size_t n) {
  size_t start = (rt_heap_used + 15) & ~(size_t)15;
  if (n > sizeof rt_heap - start) {
    return 0;
  }
  rt_heap_used = start + n;
  return rt_heap + start;
}

void *calloc(size_t count, size_t size) {
  /* No overflow check: LLVM rewrites `count > SIZE_MAX / size` into @llvm.umul.with.overflow, which
   * bennett-front refuses (itself a finding; see README). */
  void *p = malloc(count * size);
  return p ? memset(p, 0, count * size) : 0;
}

void *realloc(void *old, size_t n) {
  void *fresh = malloc(n);
  if (fresh && old) {
    /* The old block's size is not kept: copy what lies between it and the end of the heap, at most n bytes. */
    size_t available = (size_t)(rt_heap + sizeof rt_heap - (unsigned char *)old);
    memmove(fresh, old, n < available ? n : available);
  }
  return fresh;
}

void free(void *p) { (void)p; }
