/* Independent aggregate ABI witnesses. Compile this file twice, with ABI_CALLEE
 * for the definitions and without for the caller, then link. noinline keeps
 * calls at the ABI boundary. Checksums read members only: C struct padding and
 * inactive union members are never an oracle. */
#include <stdarg.h>
typedef unsigned long long u64;
typedef unsigned __int128 u128;
#define NOINLINE __attribute__((noinline))
static u64 double_bits(double x) {
  u64 b;
  __builtin_memcpy(&b, &x, 8);
  return b;
}
static unsigned float_bits(float x) {
  unsigned b;
  __builtin_memcpy(&b, &x, 4);
  return b;
}
static u128 long_bits(long double x) {
  unsigned char b[sizeof x];
  __builtin_memcpy(b, &x, sizeof x);
  u128 r = 0;
  for (unsigned i = 0; i < 10; ++i)
    r |= (u128)b[i] << (8 * i);
  return r;
}
static u128 combine(u128 a, u128 b) { return ((a << 17) | (a >> 111)) ^ b; }
static u128 chars_hash(const unsigned char *bytes, unsigned n) {
  u128 h = 0;
  for (unsigned i = 0; i < n; ++i)
    h = combine(h, bytes[i]);
  return h;
}
struct padded {
  unsigned char a;
  u64 b;
};
struct tail {
  u64 a;
  unsigned char b;
};
struct internal {
  unsigned short a;
  unsigned b;
  unsigned char c;
};
struct bits {
  u64 a : 3, b : 61;
  unsigned c : 9;
};
struct __attribute__((packed)) bits65 {
  u64 a : 64;
  unsigned b : 1;
};
struct mixed {
  double a;
  int b;
};
struct mixed_reverse {
  int a;
  double b;
};
struct float_int {
  float a;
  u64 b;
};
struct float3 {
  float a, b, c;
};
struct float4 {
  float a, b, c, d;
};
struct double2 {
  double a, b;
};
struct bools {
  _Bool a, b;
};
struct bool_mixed {
  _Bool a;
  double b;
  _Bool c;
};
struct nested {
  struct {
    unsigned a;
    unsigned char b;
  } a;
  struct {
    short a, b;
  } b;
};
union word {
  u64 a;
  double b;
};
struct union_word {
  union word a;
  unsigned char b;
};
union whole {
  u128 a;
  unsigned char b[16];
};
struct int128 {
  u128 a;
};
struct longdouble {
  long double a;
};
struct longdouble_tail {
  long double a;
  char b;
};
struct __attribute__((packed)) packed {
  unsigned char a;
  double b;
};
struct __attribute__((aligned(16))) aligned16 {
  u64 a;
  unsigned char b;
};
struct __attribute__((aligned(32))) aligned32 {
  u64 a;
  unsigned char b;
};
struct pointer {
  const u64 *a;
  u64 b;
};
typedef unsigned vector4 __attribute__((vector_size(16)));
struct vector {
  vector4 a;
};
static const u64 pointed = 0xfedcba9876543210ULL;

#ifdef ABI_CALLEE
#define SHAPE(N, INIT, HASH)                                                   \
  NOINLINE struct N make_##N(u64 x) {                                          \
    return INIT;                                                               \
  } NOINLINE struct N var_##N(unsigned tag, ...) {                             \
    va_list ap;                                                                \
    va_start(ap, tag);                                                         \
    u64 x = 0;                                                                 \
    for (unsigned i = 0; i < 8; ++i)                                           \
      x ^= va_arg(ap, u64);                                                    \
    x ^= (u64)va_arg(ap, double);                                              \
    x ^= (u64)va_arg(ap, u128);                                                \
    va_end(ap);                                                                \
    return make_##N(x ^ tag);                                                  \
  }
#else
#define SHAPE(N, INIT, HASH)                                                   \
  struct N make_##N(u64);                                                      \
  struct N var_##N(unsigned, ...);                                             \
  struct N (*volatile ptr_##N)(u64) = make_##N;                                \
  struct N (*volatile vptr_##N)(unsigned, ...) = var_##N;                      \
  static NOINLINE u128 use_##N(u64 x, unsigned mode) {                         \
    struct N p;                                                                \
    if (mode == 0)                                                             \
      p = make_##N(x);                                                         \
    else if (mode == 1)                                                        \
      p = ptr_##N(x);                                                          \
    else if (mode == 2)                                                        \
      p = var_##N(37, x, 1ULL, 2ULL, 3ULL, 4ULL, 5ULL, 6ULL, 7ULL, 13.0,       \
                  ((u128)x << 64) | 17);                                       \
    else                                                                       \
      p = vptr_##N(37, x, 1ULL, 2ULL, 3ULL, 4ULL, 5ULL, 6ULL, 7ULL, 13.0,      \
                   ((u128)x << 64) | 17);                                      \
    return HASH;                                                               \
  }
#endif
SHAPE(padded, ((struct padded){(unsigned char)x, x ^ 0xa5a5a5a5a5a5a5a5ULL}),
      combine(p.a, p.b))
SHAPE(tail, ((struct tail){x, (unsigned char)(x >> 51)}), combine(p.a, p.b))
SHAPE(internal,
      ((struct internal){(unsigned short)x, (unsigned)(x >> 11),
                         (unsigned char)(x >> 45)}),
      combine(combine(p.a, p.b), p.c))
SHAPE(bits, ((struct bits){x & 7, x >> 3, (unsigned)(x >> 55)}),
      combine(combine(p.a, p.b), p.c))
SHAPE(bits65, ((struct bits65){x, (unsigned)(x >> 63)}), combine(p.a, p.b))
SHAPE(mixed, ((struct mixed){(double)(int)(x & 65535) / 8.0, (int)(x >> 32)}),
      combine(double_bits(p.a), (unsigned)p.b))
SHAPE(mixed_reverse,
      ((struct mixed_reverse){(int)(x >> 32),
                              (double)(int)(x & 65535) / -16.0}),
      combine((unsigned)p.a, double_bits(p.b)))
SHAPE(float_int, ((struct float_int){(float)(short)x * 0.25f, ~x}),
      combine(float_bits(p.a), p.b))
SHAPE(float3,
      ((struct float3){(float)(short)x, (float)(short)(x >> 16),
                       (float)(short)(x >> 32)}),
      combine(combine(float_bits(p.a), float_bits(p.b)), float_bits(p.c)))
SHAPE(float4,
      ((struct float4){(float)(short)x, (float)(short)(x >> 16),
                       (float)(short)(x >> 32), (float)(short)(x >> 48)}),
      combine(combine(combine(float_bits(p.a), float_bits(p.b)),
                      float_bits(p.c)),
              float_bits(p.d)))
SHAPE(double2,
      ((struct double2){(double)(int)x * 0.5, (double)(int)(x >> 32) * -0.25}),
      combine(double_bits(p.a), double_bits(p.b)))
SHAPE(bools, ((struct bools){x & 1, x >> 63}), combine(p.a, p.b))
SHAPE(bool_mixed, ((struct bool_mixed){x & 1, (double)(int)x, x >> 63}),
      combine(combine(p.a, double_bits(p.b)), p.c))
SHAPE(nested,
      ((struct nested){{(unsigned)x, (unsigned char)(x >> 32)},
                       {(short)(x >> 40), (short)(x >> 48)}}),
      combine(combine(combine(p.a.a, p.a.b), (unsigned short)p.b.a),
              (unsigned short)p.b.b))
SHAPE(union_word, ((struct union_word){{.a = x}, (unsigned char)(x >> 32)}),
      combine(p.a.a, p.b))
SHAPE(int128, ((struct int128){((u128)x << 64) | ~x}), p.a)
SHAPE(longdouble, ((struct longdouble){(long double)(long long)x * 0.125L}),
      long_bits(p.a))
SHAPE(longdouble_tail,
      ((struct longdouble_tail){(long double)(long long)x * -0.25L,
                                (char)(x >> 32)}),
      combine(long_bits(p.a), (unsigned char)p.b))
SHAPE(packed, ((struct packed){(unsigned char)x, (double)(int)(x >> 32) / 4.0}),
      combine(p.a, double_bits(p.b)))
SHAPE(aligned16, ((struct aligned16){~x, (unsigned char)x}), combine(p.a, p.b))
SHAPE(aligned32, ((struct aligned32){~x, (unsigned char)x}), combine(p.a, p.b))
SHAPE(pointer, ((struct pointer){&pointed, x}), combine(*p.a, p.b))
SHAPE(vector,
      ((struct vector){{(unsigned)x, (unsigned)(x >> 32), (unsigned)~x,
                        (unsigned)(~x >> 32)}}),
      combine(combine(combine(p.a[0], p.a[1]), p.a[2]), p.a[3]))
struct chars1 {
  unsigned char a[1];
};
SHAPE(chars1, ((struct chars1){{(unsigned char)((x >> 0) + 0)}}),
      chars_hash(p.a, 1))
struct chars2 {
  unsigned char a[2];
};
SHAPE(chars2,
      ((struct chars2){
          {(unsigned char)((x >> 0) + 0), (unsigned char)((x >> 8) + 1)}}),
      chars_hash(p.a, 2))
struct chars7 {
  unsigned char a[7];
};
SHAPE(chars7,
      ((struct chars7){
          {(unsigned char)((x >> 0) + 0), (unsigned char)((x >> 8) + 1),
           (unsigned char)((x >> 16) + 2), (unsigned char)((x >> 24) + 3),
           (unsigned char)((x >> 32) + 4), (unsigned char)((x >> 40) + 5),
           (unsigned char)((x >> 48) + 6)}}),
      chars_hash(p.a, 7))
struct chars8 {
  unsigned char a[8];
};
SHAPE(chars8,
      ((struct chars8){
          {(unsigned char)((x >> 0) + 0), (unsigned char)((x >> 8) + 1),
           (unsigned char)((x >> 16) + 2), (unsigned char)((x >> 24) + 3),
           (unsigned char)((x >> 32) + 4), (unsigned char)((x >> 40) + 5),
           (unsigned char)((x >> 48) + 6), (unsigned char)((x >> 56) + 7)}}),
      chars_hash(p.a, 8))
struct chars9 {
  unsigned char a[9];
};
SHAPE(chars9,
      ((struct chars9){
          {(unsigned char)((x >> 0) + 0), (unsigned char)((x >> 8) + 1),
           (unsigned char)((x >> 16) + 2), (unsigned char)((x >> 24) + 3),
           (unsigned char)((x >> 32) + 4), (unsigned char)((x >> 40) + 5),
           (unsigned char)((x >> 48) + 6), (unsigned char)((x >> 56) + 7),
           (unsigned char)((x >> 0) + 8)}}),
      chars_hash(p.a, 9))
struct chars15 {
  unsigned char a[15];
};
SHAPE(chars15,
      ((struct chars15){
          {(unsigned char)((x >> 0) + 0), (unsigned char)((x >> 8) + 1),
           (unsigned char)((x >> 16) + 2), (unsigned char)((x >> 24) + 3),
           (unsigned char)((x >> 32) + 4), (unsigned char)((x >> 40) + 5),
           (unsigned char)((x >> 48) + 6), (unsigned char)((x >> 56) + 7),
           (unsigned char)((x >> 0) + 8), (unsigned char)((x >> 8) + 9),
           (unsigned char)((x >> 16) + 10), (unsigned char)((x >> 24) + 11),
           (unsigned char)((x >> 32) + 12), (unsigned char)((x >> 40) + 13),
           (unsigned char)((x >> 48) + 14)}}),
      chars_hash(p.a, 15))
struct chars16 {
  unsigned char a[16];
};
SHAPE(chars16,
      ((struct chars16){
          {(unsigned char)((x >> 0) + 0), (unsigned char)((x >> 8) + 1),
           (unsigned char)((x >> 16) + 2), (unsigned char)((x >> 24) + 3),
           (unsigned char)((x >> 32) + 4), (unsigned char)((x >> 40) + 5),
           (unsigned char)((x >> 48) + 6), (unsigned char)((x >> 56) + 7),
           (unsigned char)((x >> 0) + 8), (unsigned char)((x >> 8) + 9),
           (unsigned char)((x >> 16) + 10), (unsigned char)((x >> 24) + 11),
           (unsigned char)((x >> 32) + 12), (unsigned char)((x >> 40) + 13),
           (unsigned char)((x >> 48) + 14), (unsigned char)((x >> 56) + 15)}}),
      chars_hash(p.a, 16))
struct chars17 {
  unsigned char a[17];
};
SHAPE(chars17,
      ((struct chars17){
          {(unsigned char)((x >> 0) + 0), (unsigned char)((x >> 8) + 1),
           (unsigned char)((x >> 16) + 2), (unsigned char)((x >> 24) + 3),
           (unsigned char)((x >> 32) + 4), (unsigned char)((x >> 40) + 5),
           (unsigned char)((x >> 48) + 6), (unsigned char)((x >> 56) + 7),
           (unsigned char)((x >> 0) + 8), (unsigned char)((x >> 8) + 9),
           (unsigned char)((x >> 16) + 10), (unsigned char)((x >> 24) + 11),
           (unsigned char)((x >> 32) + 12), (unsigned char)((x >> 40) + 13),
           (unsigned char)((x >> 48) + 14), (unsigned char)((x >> 56) + 15),
           (unsigned char)((x >> 0) + 16)}}),
      chars_hash(p.a, 17))
struct chars31 {
  unsigned char a[31];
};
SHAPE(chars31,
      ((struct chars31){
          {(unsigned char)((x >> 0) + 0),   (unsigned char)((x >> 8) + 1),
           (unsigned char)((x >> 16) + 2),  (unsigned char)((x >> 24) + 3),
           (unsigned char)((x >> 32) + 4),  (unsigned char)((x >> 40) + 5),
           (unsigned char)((x >> 48) + 6),  (unsigned char)((x >> 56) + 7),
           (unsigned char)((x >> 0) + 8),   (unsigned char)((x >> 8) + 9),
           (unsigned char)((x >> 16) + 10), (unsigned char)((x >> 24) + 11),
           (unsigned char)((x >> 32) + 12), (unsigned char)((x >> 40) + 13),
           (unsigned char)((x >> 48) + 14), (unsigned char)((x >> 56) + 15),
           (unsigned char)((x >> 0) + 16),  (unsigned char)((x >> 8) + 17),
           (unsigned char)((x >> 16) + 18), (unsigned char)((x >> 24) + 19),
           (unsigned char)((x >> 32) + 20), (unsigned char)((x >> 40) + 21),
           (unsigned char)((x >> 48) + 22), (unsigned char)((x >> 56) + 23),
           (unsigned char)((x >> 0) + 24),  (unsigned char)((x >> 8) + 25),
           (unsigned char)((x >> 16) + 26), (unsigned char)((x >> 24) + 27),
           (unsigned char)((x >> 32) + 28), (unsigned char)((x >> 40) + 29),
           (unsigned char)((x >> 48) + 30)}}),
      chars_hash(p.a, 31))

/* C's empty result is void at the LLVM boundary; the ignored sret result still
 * has side effects. */
struct empty {};
struct wide {
  u64 a, b, c;
};
struct rec {
  u64 a, b;
};
#ifdef ABI_CALLEE
volatile u64 calls;
NOINLINE struct empty empty(u64 x) {
  calls += x;
  return (struct empty){};
}
NOINLINE struct wide ignored(u64 x) {
  calls += x;
  return (struct wide){x, ~x, x + 1};
}
NOINLINE struct rec recurse(unsigned n, u64 x) {
  if (!n)
    return (struct rec){x, ~x};
  struct rec p = recurse(n - 1, x + 3);
  return (struct rec){p.b ^ x, p.a + x};
}
NOINLINE long double scalar_long(u64 x) {
  return (long double)(long long)x / 8.0L;
}
NOINLINE union whole union_whole(u64 x) {
  return (union whole){.a = ((u128)x << 64) | ~x};
}
#else
extern volatile u64 calls;
struct empty empty(u64);
struct wide ignored(u64);
struct rec recurse(unsigned, u64);
long double scalar_long(u64);
union whole union_whole(u64);
struct wide (*volatile ignore_ptr)(u64) = ignored;
static u128 special(u64 x, unsigned mode) {
  if (mode == 0) {
    calls = 7;
    empty(x);
    ignored(x);
    ignore_ptr(x);
    return calls;
  }
  if (mode == 1) {
    struct rec p = recurse(7, x);
    return combine(p.a, p.b);
  }
  if (mode == 2)
    return long_bits(scalar_long(x));
  return union_whole(x).a;
}

u128 result(unsigned which, u64 x, unsigned mode) {
  switch (which) {
  case 0:
    return use_padded(x, mode);
  case 1:
    return use_tail(x, mode);
  case 2:
    return use_internal(x, mode);
  case 3:
    return use_bits(x, mode);
  case 4:
    return use_bits65(x, mode);
  case 5:
    return use_mixed(x, mode);
  case 6:
    return use_mixed_reverse(x, mode);
  case 7:
    return use_float_int(x, mode);
  case 8:
    return use_float3(x, mode);
  case 9:
    return use_float4(x, mode);
  case 10:
    return use_double2(x, mode);
  case 11:
    return use_bools(x, mode);
  case 12:
    return use_bool_mixed(x, mode);
  case 13:
    return use_nested(x, mode);
  case 14:
    return use_union_word(x, mode);
  case 15:
    return use_int128(x, mode);
  case 16:
    return use_longdouble(x, mode);
  case 17:
    return use_longdouble_tail(x, mode);
  case 18:
    return use_packed(x, mode);
  case 19:
    return use_aligned16(x, mode);
  case 20:
    return use_aligned32(x, mode);
  case 21:
    return use_pointer(x, mode);
  case 22:
    return use_vector(x, mode);
  case 23:
    return use_chars1(x, mode);
  case 24:
    return use_chars2(x, mode);
  case 25:
    return use_chars7(x, mode);
  case 26:
    return use_chars8(x, mode);
  case 27:
    return use_chars9(x, mode);
  case 28:
    return use_chars15(x, mode);
  case 29:
    return use_chars16(x, mode);
  case 30:
    return use_chars17(x, mode);
  case 31:
    return use_chars31(x, mode);
  default:
    return special(x, mode);
  }
}
unsigned cases(void) { return 33; }

#if defined(ABI_NATIVE) || defined(ABI_MUSL)
int printf(const char *, ...);
static const u64 inputs[] = {0,
                             1,
                             7,
                             0x7fffffffffffffffULL,
                             0x8000000000000000ULL,
                             0xffffffffffffffffULL,
                             0x0123456789abcdefULL,
                             0xfedcba9876543210ULL};
int main(void) {
  for (unsigned i = 0; i < cases(); ++i)
    for (unsigned mode = 0; mode < 4; ++mode)
      for (unsigned j = 0; j < sizeof inputs / sizeof inputs[0]; ++j) {
        u128 r = result(i, inputs[j], mode);
        printf("%016llx%016llx\n", (u64)(r >> 64), (u64)r);
      }
  return 0;
}
#endif
#endif
