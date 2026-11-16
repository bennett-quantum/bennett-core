/* Raw encodings end to end: native C versus clang -> front -> executor, at O0/O2.
 * Runtime-native probes isolate the library; these also exercise predicate selection, bit operations,
 * class tests, narrow integer conversion, LLVM optimizations and reuse of operation results. */
typedef unsigned long long U64;
typedef unsigned __int128 U;
static volatile unsigned short E[] = {
  0,0x8000,0,0x8000,1,0x8001,0x3fff,0xbfff,0x7ffe,0xfffe,0x7fff,0xffff,
  0x7fff,0xffff,0,0x8000,0x3fff,0xbfff,0x7fff,0xffff,
};
static volatile U64 M[] = {
  0x8000000000000000ULL,0xffffffffffffffffULL,0xc000000000000001ULL,0x8000000000000001ULL,
  0,1,0x4000000000000000ULL,0x7fffffffffffffffULL,1,0x4000000000000000ULL,
  0,0,0x4000000000000001ULL,1,
  0,0,0x8000000000000000ULL,0x8000000000000000ULL,0xc000000000000123ULL,0x8000000000000456ULL,
};
#define N (sizeof(E)/sizeof(*E))
static long double x(unsigned i) {
  long double r=0; U64 m=M[i]; unsigned short e=E[i];
  __builtin_memcpy(&r,&m,8); __builtin_memcpy((char *)&r+8,&e,2); return r;
}
static U bits(long double v) {
  U64 m; unsigned short e;
  __builtin_memcpy(&m,&v,8); __builtin_memcpy(&e,(char *)&v+8,2); return (U)e<<64|m;
}
static U unary(unsigned op, long double a) {
  switch(op) {
  case 0:return bits(-a);
  case 1:return bits(__builtin_fabsl(a));
  case 2:return bits(__builtin_copysignl(a,-1.L));
  case 3:return bits(__builtin_sqrtl(a));
  case 4:return bits(__builtin_floorl(a));
  case 5:return bits(__builtin_ceill(a));
  case 6:return bits(__builtin_truncl(a));
  case 7:return bits(__builtin_rintl(a));
  case 8:return bits(__builtin_nearbyintl(a));
  case 9:return (U64)(signed char)a;
  case 10:return (unsigned char)a;
  case 11:return (U64)(short)a;
  case 12:return (unsigned short)a;
  case 13:return (U64)(int)a;
  case 14:return (unsigned)a;
  case 15:return (U64)(long long)a;
  case 16:return (U64)a;
  case 17:return (U)(__int128)a;
  case 18:return (U)a;
  case 19:{float f=a;unsigned u;__builtin_memcpy(&u,&f,4);return u;}
  case 20:{double d=a;U64 u;__builtin_memcpy(&u,&d,8);return u;}
  case 21:return (U)__builtin_isnan(a) | (U)__builtin_isinf(a)<<1 | (U)__builtin_isfinite(a)<<2 |
    (U)__builtin_isnormal(a)<<3 | (U)(__builtin_signbit(a)!=0)<<4;
  case 22:return __builtin_fpclassify(0,1,2,3,4,a);
  case 23:return (U)__builtin_isfpclass(a,0x001) | (U)__builtin_isfpclass(a,0x002)<<1 |
    (U)__builtin_isfpclass(a,0x060)<<2 | (U)__builtin_isfpclass(a,0x090)<<3;
  case 24:return __builtin_issignaling(a);
  case 25:return bits(__builtin_powil(a,3));
  case 26:return bits(__builtin_powil(a,-2));
  case 27:return (_Bool)a;
  case 28:return (U64)__builtin_lrintl(a);
  case 29:return (U64)__builtin_llrintl(a);
  case 30:return bits((long double)(_Float16)a);
  default:return bits((long double)(float)a);
  }
}
#define UNARY 32
#define BINARY 10
static U binary(unsigned op,long double a,long double b) {
  switch(op) {
  case 0:return bits(a+b);
  case 1:return bits(a-b);
  case 2:return bits(a*b);
  case 3:return bits(a/b);
  case 4:return (U)(a==b) | (U)(a!=b)<<1 | (U)(a<b)<<2 | (U)(a<=b)<<3 | (U)(a>b)<<4 |
    (U)(a>=b)<<5 | (U)__builtin_isunordered(a,b)<<6 | (U)!__builtin_isunordered(a,b)<<7 |
    (U)__builtin_isless(a,b)<<8 | (U)__builtin_islessequal(a,b)<<9 | (U)__builtin_isgreater(a,b)<<10 |
    (U)__builtin_isgreaterequal(a,b)<<11 | (U)__builtin_islessgreater(a,b)<<12 |
    (U)(!(a<b))<<13 | (U)(!(a<=b))<<14 | (U)(!(a>b))<<15 | (U)(!(a>=b))<<16;
  case 5:return bits(__builtin_fminl(a,b));
  case 6:return bits(__builtin_fmaxl(a,b));
  case 7:return bits((a+b)*b);
  case 8:return bits(__builtin_fminl(a,b)+b);
  default:return bits(__builtin_fmaxl(a,b)*a);
  }
}
unsigned results(void) {return UNARY*N+BINARY*N*N;}
U result(unsigned i) {
  if(i<UNARY*N)return unary(i/N,x(i%N));
  i-=UNARY*N;return binary(i/(N*N),x(i/N%N),x(i%N));
}
#ifdef NATIVE
int printf(const char *,...);
int main(void) {
 for(unsigned i=0;i<results();i++) {U v=result(i); printf("%04llx%016llx\n",(U64)(v>>64),(U64)v);}
 return 0;
}
#endif
