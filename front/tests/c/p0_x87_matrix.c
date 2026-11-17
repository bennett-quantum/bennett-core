/* Independent native/runtime comparison. Native expressions read volatile operands and use the
 * platform's x87/glibc/libgcc implementations; rt_* symbols are the actual runtime bitcode, compiled
 * natively with renamed symbols. No copy of the runtime's encoding classifier is used as the oracle.
 * Raw 80-bit equality excludes six ABI padding bytes. Out-of-range integer conversions are measured
 * as target behavior, not claimed to be defined C. Noncanonical fmal/roundl/half truncation are known debts.
 * Mode `rounding` puts glibc's software roundevenl against the runtime on every 16-bit sign and exponent.
 */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef unsigned long long U;
typedef __int128 I128;
typedef unsigned __int128 U128;
static volatile long double a, b;
static volatile int power;
static unsigned checked, missed;
static const char *current;
static unsigned ai, bi;
static long double values[512];
static unsigned count;
static void add(unsigned e, U m) {
  long double v = 0;
  uint16_t se = e;
  memcpy(&v, &m, 8);
  memcpy((char *)&v + 8, &se, 2);
  memcpy(&values[count++], &v, 16);
}
static void check(const char *name, const void *n, const void *d, unsigned size) {
  checked++;
  if (!memcmp(n, d, size)) return;
  missed++;
  if (missed <= 24) {
    printf("%s/%s a=%u b=%u native=", current, name, ai, bi);
    for (unsigned i = size; i; i--) printf("%02x", ((const unsigned char *)n)[i-1]);
    printf(" runtime=");
    for (unsigned i = size; i; i--) printf("%02x", ((const unsigned char *)d)[i-1]);
    printf("\n");
  }
}
#define CHECK(name,T,native,device,size) do { T nv=(native), dv=(device); check(name,&nv,&dv,size); } while(0)
#define LD(name,n,d) CHECK(name,long double,n,d,10)
long double rt___addxf3(long double,long double);
long double rt___subxf3(long double,long double);
long double rt___mulxf3(long double,long double);
long double rt___divxf3(long double,long double);
long rt___eqxf2(long double,long double);
long rt___nexf2(long double,long double);
long rt___ltxf2(long double,long double);
long rt___lexf2(long double,long double);
long rt___gtxf2(long double,long double);
long rt___gexf2(long double,long double);
long rt___unordxf2(long double,long double);
long double rt_sqrtl(long double);
long double rt_floorl(long double);
long double rt_ceill(long double);
long double rt_truncl(long double);
long double rt_rintl(long double);
long double rt_nearbyintl(long double);
long double rt_roundevenl(long double);
long double rt_roundl(long double);
long double rt_fmal(long double,long double,long double);
long double rt_fminl(long double,long double);
long double rt_fmaxl(long double,long double);
long double rt___powixf2(long double,int);
int rt___fixxfsi(long double);
long long rt___fixxfdi(long double);
I128 rt___fixxfti(long double);
unsigned rt___fixunsxfsi(long double);
U rt___fixunsxfdi(long double);
U128 rt___fixunsxfti(long double);
float rt___truncxfsf2(long double);
long double rt___extendsfxf2(float);
double rt___truncxfdf2(long double);
long double rt___extenddfxf2(double);
long double rt___floatsixf(int);
long double rt___floatdixf(long long);
long double rt___floattixf(I128);
long double rt___floatunsixf(unsigned);
long double rt___floatundixf(U);
long double rt___floatuntixf(U128);
long double rt___extendhfxf2(_Float16);
_Float16 rt___truncxfhf2(long double);
__bf16 rt___truncxfbf2(long double);
__bf16 __truncxfbf2(long double);

static void setup(void) {
  /* Cartesian exponent/significand set includes every encoding class, sign, quiet/payload bit,
     exponent extremes and several ordinary rounding and integer conversion boundaries. */
  unsigned es[] = {0,1,2,0x3ffe,0x3fff,0x403e,0x407e,0x7ffe,0x7fff};
  U ms[] = {0,1,0x3fffffffffffffffULL,0x4000000000000000ULL,0x7fffffffffffffffULL,
            0x8000000000000000ULL,0x8000000000000001ULL,0xc000000000000001ULL,~0ULL};
  for (unsigned s=0;s<2;s++) for(unsigned e=0;e<sizeof(es)/sizeof(*es);e++)
    for(unsigned m=0;m<sizeof(ms)/sizeof(*ms);m++) add(es[e]|s*0x8000,ms[m]);
  U r=0xb7c159e07ad6e33bULL;
  for(unsigned i=0;i<64;i++) {r^=r<<13;r^=r>>7;r^=r<<17;add((r>>48)&0xffff,r);}
  /* Canonical finite randoms: separately ensure ordinary arithmetic is not changed by operand checks. */
  for(unsigned i=0;i<128;i++) {r^=r<<13;r^=r>>7;r^=r<<17;
    unsigned e=(r>>48)&0xffff; if((e&0x7fff)==0x7fff)e^=1;
    add(e,(e&0x7fff)?r|0x8000000000000000ULL:r&0x7fffffffffffffffULL);}
}
static void unary(void) {
  LD("sqrt",sqrtl(a),rt_sqrtl(a));
  LD("floor",floorl(a),rt_floorl(a));
  LD("ceil",ceill(a),rt_ceill(a));
  LD("trunc",truncl(a),rt_truncl(a));
  LD("rint",rintl(a),rt_rintl(a));
  LD("nearbyint",nearbyintl(a),rt_nearbyintl(a));
  /* glibc roundevenl is software on this target, unlike rintl; the runtime follows it on every encoding.
     roundl, fmal and the half/bfloat truncations are software too and still differ on noncanonical
     encodings (known debts), so only canonical inputs meet them here. */
  uint16_t e; U m;
  memcpy(&e,(const char *)&a+8,2); memcpy(&m,(const void *)&a,8);
  LD("roundeven",__builtin_roundevenl(a),rt_roundevenl(a));
  if (((e & 0x7fff) == 0 && !(m >> 63)) || ((e & 0x7fff) != 0 && (m >> 63)))
  {
    LD("round",roundl(a),rt_roundl(a));
    LD("fma-x11",fmal(a,1.L,1.L),rt_fmal(a,1.L,1.L));
    LD("fma-11x",fmal(1.L,1.L,a),rt_fmal(1.L,1.L,a));
    CHECK("to-hf",_Float16,(_Float16)a,rt___truncxfhf2(a),2);
    CHECK("to-bf",__bf16,__truncxfbf2(a),rt___truncxfbf2(a),2);
  }
  CHECK("to-si",int,(int)a,rt___fixxfsi(a),sizeof(int));
  CHECK("to-di",long long,(long long)a,rt___fixxfdi(a),sizeof(long long));
  CHECK("to-ti",I128,(I128)a,rt___fixxfti(a),sizeof(I128));
  CHECK("to-unssi",unsigned,(unsigned)a,rt___fixunsxfsi(a),sizeof(unsigned));
  CHECK("to-unsdi",U,(U)a,rt___fixunsxfdi(a),sizeof(U));
  CHECK("to-unsti",U128,(U128)a,rt___fixunsxfti(a),sizeof(U128));
  CHECK("to-sf",float,(float)a,rt___truncxfsf2(a),sizeof(float));
  LD("via-sf",(long double)(float)a,rt___extendsfxf2(rt___truncxfsf2(a)));
  CHECK("to-df",double,(double)a,rt___truncxfdf2(a),sizeof(double));
  LD("via-df",(long double)(double)a,rt___extenddfxf2(rt___truncxfdf2(a)));
}
static void binary(void) {
  LD("add",a+b,rt___addxf3(a,b));
  LD("add-chain",(a+b)+a,rt___addxf3(rt___addxf3(a,b),a));
  LD("sub",a-b,rt___subxf3(a,b));
  LD("sub-chain",(a-b)+a,rt___addxf3(rt___subxf3(a,b),a));
  LD("mul",a*b,rt___mulxf3(a,b));
  LD("mul-chain",(a*b)+a,rt___addxf3(rt___mulxf3(a,b),a));
  LD("div",a/b,rt___divxf3(a,b));
  LD("div-chain",(a/b)+a,rt___addxf3(rt___divxf3(a,b),a));
  CHECK("eq",int,a==b,rt___eqxf2(a,b)==0,sizeof(int));
  CHECK("ne",int,a!=b,rt___nexf2(a,b)!=0,sizeof(int));
  CHECK("lt",int,a<b,rt___ltxf2(a,b)<0,sizeof(int));
  CHECK("le",int,a<=b,rt___lexf2(a,b)<=0,sizeof(int));
  CHECK("gt",int,a>b,rt___gtxf2(a,b)>0,sizeof(int));
  CHECK("ge",int,a>=b,rt___gexf2(a,b)>=0,sizeof(int));
  CHECK("unord",int,__builtin_isunordered(a,b),rt___unordxf2(a,b)!=0,sizeof(int));
  LD("fmin",fminl(a,b),rt_fminl(a,b));
  LD("fmin-chain",fminl(a,b)+a,rt___addxf3(rt_fminl(a,b),a));
  LD("fmax",fmaxl(a,b),rt_fmaxl(a,b));
  LD("fmax-chain",fmaxl(a,b)+a,rt___addxf3(rt_fmaxl(a,b),a));
}
static void powers(void) {
  int ns[]={-17,-4,-3,-2,-1,0,1,2,3,4,17};
  for(unsigned i=0;i<sizeof(ns)/sizeof(*ns);i++) {
    power=ns[i]; bi=i;
    /* Exponent 1 included: libgcc returns the argument's bits without arithmetic. */
    LD("powi",__builtin_powil(a,power),rt___powixf2(a,power));
  }
}
static void integers(void) {
  U128 us[]={0,1,127,128,255,256,32767,32768,65535,65536,((U128)1<<31)-1,
    (U128)1<<31,((U128)1<<32)-1,((U128)1<<63)-1,(U128)1<<63,((U128)1<<64)-1,
    (U128)1<<64,((U128)1<<127)-1,(U128)1<<127,~(U128)0};
  for(unsigned i=0;i<sizeof(us)/sizeof(*us);i++) {
    volatile U128 u=us[i]; volatile I128 s=(I128)us[i]; ai=i;
    LD("from-si",(long double)(int)s,rt___floatsixf((int)s));
    LD("from-di",(long double)(long long)s,rt___floatdixf((long long)s));
    LD("from-ti",(long double)(I128)s,rt___floattixf((I128)s));
    LD("from-unsi",(long double)(unsigned)u,rt___floatunsixf((unsigned)u));
    LD("from-undi",(long double)(U)u,rt___floatundixf((U)u));
    LD("from-unti",(long double)(U128)u,rt___floatuntixf((U128)u));
  }
}
static void floating(void) {
  /* All half/bfloat source bit patterns, including signed NaNs and every subnormal. */
  for(unsigned i=0;i<65536;i++) {
    uint16_t bits=i; _Float16 h; __bf16 bf; ai=i;
    memcpy(&h,&bits,2); memcpy(&bf,&bits,2);
    volatile _Float16 vh=h; volatile __bf16 vb=bf;
    LD("from-hf",(long double)vh,rt___extendhfxf2(vh));
    /* LLVM extends bfloat via float (an exact shift); the runtime has no bfloat->x80 entry. */
    unsigned word=(unsigned)bits<<16; float sf; memcpy(&sf,&word,4);
    LD("from-bf",(long double)vb,rt___extendsfxf2(sf));
  }
  U r=0x831df356a75c238bULL;
  for(unsigned i=0;i<4096;i++) {
    r^=r<<13;r^=r>>7;r^=r<<17; ai=i;
    /* Two payload samples per double exponent, a spread of float exponents and signs. */
    U word=(r&0x800fffffffffffffULL)|((U)(i%2048)<<52);
    double d; memcpy(&d,&word,8); volatile double vd=d;
    unsigned fw=(unsigned)r; float f;memcpy(&f,&fw,4);volatile float vf=f;
    LD("from-df",(long double)vd,rt___extenddfxf2(vd));
    LD("from-sf",(long double)vf,rt___extendsfxf2(vf));
  }
}
static void rounding(void) {
  /* glibc's software rounding against the runtime on every exponent, both signs, with significands that
     put a set bit, a half or a carry at each place the binary point can be (noncanonical ones included:
     without the integer bit, or with it at exponent 0). */
  U ms[]={0,1,2,3,0x7fffffffULL,0x80000000ULL,0x80000001ULL,0xffffffffULL,0x100000000ULL,0x180000000ULL,
          0x3fffffffffffffffULL,0x4000000000000000ULL,0x4000000000000001ULL,0x7fffffffffffffffULL,
          0x8000000000000000ULL,0x8000000000000001ULL,0x8000000080000000ULL,0x8000000100000000ULL,
          0xc000000000000000ULL,0xc000000000000001ULL,0xfffffffeffffffffULL,~0ULL};
  U r=0x2545f4914f6cdd1dULL;
  for(unsigned e=0;e<0x10000;e++) {
    ai=e;
    for(unsigned m=0;m<sizeof(ms)/sizeof(*ms)+4;m++) {
      U bits;
      if(m<sizeof(ms)/sizeof(*ms)) bits=ms[m];
      else {r^=r<<13;r^=r>>7;r^=r<<17;bits=r;}
      long double v=0; uint16_t se=e; bi=m;
      memcpy(&v,&bits,8); memcpy((char *)&v+8,&se,2); memcpy((void *)&a,&v,16);
      LD("roundeven",__builtin_roundevenl(a),rt_roundevenl(a));
    }
  }
}
int main(int argc,char **argv) {
  setup(); current=argc>1?argv[1]:"matrix";
  if(!strcmp(current,"integers")) integers();
  else if(!strcmp(current,"rounding")) rounding();
  else if(!strcmp(current,"floating")) floating();
  else for(ai=0;ai<count;ai++) {
    memcpy((void *)&a,&values[ai],16);
    if(!strcmp(current,"powers")) {powers();continue;}
    unary();
    for(bi=0;bi<count;bi++) {memcpy((void *)&b,&values[bi],16);binary();}
  }
  printf("%s checked=%u mismatches=%u\n",current,checked,missed);
  return missed?1:0;
}
