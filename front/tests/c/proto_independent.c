/* Independent native oracle: register classes and spills, whole memory values, result registers,
 * signedness and widths, tables, recursion and old-style definitions. Never compare padding or addresses.
 * mode 0: cast call; 1: volatile pointer; 2: table; 3: unprototyped pointer; 4: matching prototype. */
typedef unsigned long U;
typedef unsigned __int128 W;
#define NI __attribute__((noinline))
#define SHAPES 24
#define MODES 5
#define CALL(R, P, F, ...) \
  (mode == 0 ? ((R (*)P)F)(__VA_ARGS__) \
   : mode == 1 ? ({ R (*volatile p)P = (R (*)P)F; p(__VA_ARGS__); }) \
   : mode == 2 ? ({ R (*volatile t[2])P = {(R (*)P)F, (R (*)P)F}; t[x & 1](__VA_ARGS__); }) \
   : ({ R (*volatile p)() = (R (*)())F; p(__VA_ARGS__); }))

NI U mixed(U a,U b,U c,U d,U e,U f,U g,U h,double i,double j,double k,double l,double m,double n,double o,double p,double q,double r) {
  return a+3*b+5*c+7*d+11*e+13*f+17*g+19*h+(U)i+3*(U)j+5*(U)k+7*(U)l+11*(U)m+13*(U)n+17*(U)o+19*(U)p+23*(U)q+29*(U)r;
}
NI U missing(U x, U y, double unread) { (void)unread; return x+13*y; }
#define INTEGER(F,T) NI U F(T x) { return (U)x*13+7; }
INTEGER(i8,signed char)
INTEGER(u8,unsigned char)
INTEGER(i16,short)
INTEGER(u16,unsigned short)
INTEGER(i32,int)
INTEGER(u32,unsigned)
INTEGER(i64,long)
INTEGER(u64,U)
NI W i128(W x) { return x*13+(x>>71); }
NI __int128 signed128(__int128 x) { return x+17; }
struct B { U a[3]; };
NI U memory(U a,U b,U c,U d,U e,U f,struct B x,U y) { return x.a[0]+3*x.a[1]+7*x.a[2]+13*y; }
NI long double extended(U a,U b,U c,U d,U e,U f,U g,long double x,U y) { return x*3.0L+(long double)(g+y); }
NI struct B memory_result(U x) { struct B b={{x,x+3,x+9}}; return b; }
struct GG { U a,b; };
struct GS { U a; double b; };
struct SG { double a; U b; };
struct SS { double a,b; };
NI struct GG pair(U x) { struct GG s={x,x+17}; return s; }
NI struct GS pair_mixed(U x) { struct GS s={x,7.0}; return s; }
NI struct SS pair_sse(U x) { struct SS s={(double)(x&255),7.0}; return s; }
struct FF { float a,b; };
NI U float_pair(struct FF x) { return (U)x.a+13*(U)x.b; }
NI U sum(U n,...) {
  __builtin_va_list ap; __builtin_va_start(ap,n); U r=0;
  for(U i=0;i<n;i++)r=r*13+__builtin_va_arg(ap,U);
  __builtin_va_end(ap); return r;
}
NI U fixed(U a,U b,U c,U d,U e,U f,U g,U h) { return a+3*b+5*c+7*d+11*e+13*f+17*g+19*h; }
NI U recursive(unsigned n) { return n ? n+((U (*)(U,U))recursive)((U)(n-1),(U)99) : 0; }
NI U old_style(a,b) float a;short b; { return (U)a+13*(U)b; }
NI unsigned table_a(unsigned x) { return x*7+3; }
NI unsigned table_b(unsigned short x) { return x*13+9; }
NI unsigned table_c(U x) { return (unsigned)x*17+1; }
static unsigned (*volatile table[3])(U) = {
  (unsigned (*)(U))table_a, (unsigned (*)(U))table_b, table_c
};

U result(unsigned shape,U x,unsigned mode) {
  U v=x&127;
  switch(shape) {
  case 0: {
    /* Different order, same independently allocated register sequences; both classes spill.
     * Use a pointer even in mode 0: direct class swaps have a documented conservative refusal. */
    U (*volatile p)(double,U,double,U,double,U,double,U,double,U,double,U,double,U,double,U,double,double,U) =
      (U (*)(double,U,double,U,double,U,double,U,double,U,double,U,double,U,double,U,double,double,U))mixed;
    if(mode==4)return mixed(v,1,2,3,4,5,6,7,2,3,4,5,6,7,8,9,10,11);
    return p(2,v,3,(U)1,4,(U)2,5,(U)3,6,(U)4,7,(U)5,8,(U)6,9,(U)7,10,11,(U)99);
  }
  case 1: return mode==4 ? missing(x,7,0) : CALL(U,(U,U),missing,x,(U)7);
  case 2: { int a=(int)v-64; return mode==4 ? i8((signed char)a) : CALL(U,(short,U),i8,(short)a,(U)99); }
  case 3: return mode==4 ? u8((unsigned char)v) : CALL(U,(unsigned short,U),u8,(unsigned short)v,(U)99);
  case 4: { int a=(int)(x&32767)-16384; return mode==4 ? i16((short)a) : CALL(U,(int,U),i16,a,(U)99); }
  case 5: return mode==4 ? u16((unsigned short)x) : CALL(U,(unsigned,U),u16,(unsigned)(unsigned short)x,(U)99);
  case 6: return mode==4 ? i32((int)x) : CALL(U,(U,U),i32,x,(U)99);
  case 7: return mode==4 ? u32((unsigned)x) : CALL(U,(U,U),u32,x,(U)99);
  case 8: return mode==4 ? i64((long)x) : CALL(U,(U,U),i64,x,(U)99);
  case 9: return mode==4 ? u64(x) : CALL(U,(long,U),u64,(long)x,(U)99);
  case 10: {
    W w=(W)x|((W)~x<<64);
    return mode==4 ? (U)i128(w) : CALL(U,(U,U,U),i128,x,~x,(U)99);
  }
  case 11: return mode==4 ? (U)signed128((__int128)v-64) : CALL(U,(__int128,U),signed128,(__int128)v-64,(U)99);
  case 12: {
    struct B b={{x,x+3,x+9}};
    return mode==4 ? memory(1,2,3,4,5,6,b,7) : CALL(U,(U,U,U,U,U,U,struct B,U,U),memory,(U)1,(U)2,(U)3,(U)4,(U)5,(U)6,b,(U)7,(U)99);
  }
  case 13: return mode==4 ? (U)extended(1,2,3,4,5,6,7,(long double)v+0.25L,11) : (U)CALL(long double,(U,U,U,U,U,U,U,long double,U,U),extended,(U)1,(U)2,(U)3,(U)4,(U)5,(U)6,(U)7,(long double)v+0.25L,(U)11,(U)99);
  case 14: { struct B b= mode==4 ? memory_result(x) : CALL(struct B,(U,U),memory_result,x,(U)99); return b.a[0]+3*b.a[1]+7*b.a[2]; }
  case 15: { W w; if(mode==4){struct GG s=pair(x);w=(W)s.a|((W)s.b<<64);}else w=CALL(W,(U),pair,x); return (U)w+3*(U)(w>>64); }
  case 16: { struct SG s; if(mode==4){struct GS t=pair_mixed(x);s.a=t.b;s.b=t.a;}else s=CALL(struct SG,(U),pair_mixed,x);return (U)s.a+3*s.b; }
  case 17: return mode==4 ? (U)pair_sse(x).a : (U)CALL(double,(U),pair_sse,x);
  case 18: {
    union {double d;struct FF f;} bits; bits.f.a=(float)v;bits.f.b=7.0f;
    U (*volatile p)(double)=(U (*)(double))float_pair;
    return mode==4 ? float_pair(bits.f) : p(bits.d);
  }
  case 19: return mode==4 ? sum(8,x,(U)1,(U)2,(U)3,(U)4,(U)5,(U)6,(U)7) : CALL(U,(U,U,U,U,U,U,U,U,U),sum,(U)8,x,(U)1,(U)2,(U)3,(U)4,(U)5,(U)6,(U)7);
  case 20: {
    U (*volatile p)(U,...)=(U (*)(U,...))fixed;
    return mode==4 ? fixed(x,1,2,3,4,5,6,7) : p(x,(U)1,(U)2,(U)3,(U)4,(U)5,(U)6,(U)7);
  }
  case 21: return mode==4 ? recursive((unsigned)(v&15)) : CALL(U,(U,U),recursive,v&15,(U)99);
  case 22: return mode==4 ? old_style((float)v,(short)7) : CALL(U,(double,int,U),old_style,(double)v,7,(U)99);
  case 23: { U r=0; for(U i=0;i<3;i++) { unsigned y=mode==4 ? (i==0 ? table_a((unsigned)v) : i==1 ? table_b((unsigned short)v) : table_c(v)) : table[i](v); r=r*31+y; } return r; }
  default:return 0;
  }
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  const U inputs[]={0,1,7,0x7fffffffffffffffUL,0x8000000000000000UL,~0UL,0x0123456789abcdefUL,0xfedcba9876543210UL};
  for(unsigned k=0;k<SHAPES;k++)for(unsigned m=0;m<MODES;m++)for(unsigned i=0;i<8;i++)printf("%lx\n",result(k,inputs[i],m));
}
#endif
