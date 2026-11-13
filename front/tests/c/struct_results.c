/* struct_results.c: small structs returned by value, which clang returns as LLVM aggregates in registers
 * ({ i64, i64 }, { i64, i32 }, { i8, i64 }, ...). Returns 0, or the number of the first check that
 * failed. Fields are distinct and of mixed width and sign, so that a field read from the wrong offset,
 * at the wrong width or with the wrong extension changes a result; calls go directly, through a pointer,
 * recursively and in a loop. */
struct pair { long a, b; };            /* { i64, i64 } */
struct three { int x, y, z; };         /* { i64, i32 } */
struct mixed { char c; long l; };      /* { i8, i64 } */
struct ptrlen { const char *p; long n; };
struct packed6 { short s[3]; long l; }; /* { i64, i64 } with a short array inside */
struct odd { char c[11]; };            /* { i64, i24 } */

__attribute__((noinline)) struct pair make_pair(long a, long b) { return (struct pair){b - a, a * b}; }
__attribute__((noinline)) struct three make_three(int v) { return (struct three){-v, v * 2, v - 7}; }
__attribute__((noinline)) struct mixed make_mixed(int c) { return (struct mixed){(char)c, -1000000000000L - c}; }
__attribute__((noinline)) struct ptrlen find(const char *s, char c) {
  long i = 0;
  while (s[i] && s[i] != c) i++;
  return (struct ptrlen){s + i, i};
}
__attribute__((noinline)) struct packed6 make_packed(short a) { return (struct packed6){{a, (short)-a, 3}, 77}; }
__attribute__((noinline)) struct odd make_odd(char base) {
  struct odd o;
  for (int i = 0; i < 11; i++) o.c[i] = (char)(base + i * 3);
  return o;
}

/* A struct result chosen at run time, and passed through. */
__attribute__((noinline)) struct pair pick(int which, struct pair x, struct pair y) { return which ? x : y; }
__attribute__((noinline)) struct pair fib(int n) {
  if (n == 0) return (struct pair){0, 1};
  struct pair p = fib(n - 1);
  return (struct pair){p.b, p.a + p.b};
}

struct pair (*volatile through)(long, long) = make_pair;

int main(void) {
  struct pair p = make_pair(3, -4);
  if (p.a != -7 || p.b != -12) return 1;
  struct three t = make_three(5);
  if (t.x != -5 || t.y != 10 || t.z != -2) return 2;
  struct mixed m = make_mixed(-3);
  if (m.c != -3 || m.l != -1000000000000L + 3) return 3;
  const char *text = "hello, world";
  struct ptrlen f = find(text, ',');
  if (f.p != text + 5 || f.n != 5) return 4;
  struct packed6 k = make_packed(-9);
  if (k.s[0] != -9 || k.s[1] != 9 || k.s[2] != 3 || k.l != 77) return 5;
  struct odd o = make_odd(-5);
  for (int i = 0; i < 11; i++)
    if (o.c[i] != (char)(-5 + i * 3)) return 6;
  struct pair q = through(10, 2);
  if (q.a != -8 || q.b != 20) return 7;
  if (pick(1, p, q).a != -7 || pick(0, p, q).b != 20) return 8;
  if (fib(40).a != 102334155) return 9;
  long sum = 0;
  for (int i = 0; i < 50; i++) sum += make_pair(i, 2).b - make_three(i).z;
  if (sum != 2 * 1225 - (1225 - 7 * 50)) return 10;
  return 0;
}
