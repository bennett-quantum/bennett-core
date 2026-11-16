/* Independent mixed varargs. Prefix length varies both register classes, then
 * overflow arguments alternate large alignments and 8/16-byte stack slots.
 * Copies are made after consuming part of the list and forwarded twice. */
#include <stdarg.h>
typedef unsigned long U;
typedef unsigned __int128 W;
typedef int V4 __attribute__((vector_size(16)));
typedef int V8 __attribute__((vector_size(32)));
typedef int V16 __attribute__((vector_size(64)));
struct A { U x,y,z; } __attribute__((aligned(32)));
struct B { U x; } __attribute__((aligned(64)));
struct C { U x; } __attribute__((aligned(256)));
#ifndef PAGE_ALIGN
#define PAGE_ALIGN 4096
#endif
struct P { U x; } __attribute__((aligned(PAGE_ALIGN)));
#define NI __attribute__((noinline))
static U mix(U h,U x) { return h*65599+x; }
NI U rest(va_list ap) {
  U h=3;
  V4 v=va_arg(ap,V4); for(int i=0;i<4;i++) h=mix(h,(unsigned)v[i]);
  struct A a=va_arg(ap,struct A); h=mix(h,a.x); h=mix(h,a.y); h=mix(h,a.z);
  h=mix(h,(U)(va_arg(ap,double)*8));
  for(int i=0;i<7;i++) h=mix(h,va_arg(ap,U));
  for(int i=0;i<9;i++) h=mix(h,(U)(va_arg(ap,double)*8));
  W w=va_arg(ap,W); h=mix(mix(h,(U)(w>>64)),(U)w);
  struct B b=va_arg(ap,struct B); h=mix(h,b.x);
  long double ld=va_arg(ap,long double); h=mix(h,(U)(ld*16));
  V8 v8=va_arg(ap,V8); for(int i=0;i<8;i++) h=mix(h,(unsigned)v8[i]);
  h=mix(h,va_arg(ap,U));
  struct C c=va_arg(ap,struct C); h=mix(h,c.x);
  V16 v16=va_arg(ap,V16); for(int i=0;i<16;i++) h=mix(h,(unsigned)v16[i]);
  struct P p=va_arg(ap,struct P); h=mix(h,p.x);
  h=mix(h,(U)va_arg(ap,int));
  return h;
}
NI U consume(struct B named,unsigned count,...) {
  va_list ap,a,b;
  va_start(ap,count);
  U prefix=named.x;
  for(unsigned i=0;i<count;i++) {
    prefix=mix(prefix,va_arg(ap,U));
    prefix=mix(prefix,(U)(va_arg(ap,double)*8));
  }
  va_copy(a,ap); va_copy(b,ap);
  U x=rest(ap),y=rest(a),z=rest(b);
  va_end(a); va_end(b); va_end(ap);
  return x==y && y==z ? mix(prefix,x) : ~0UL;
}
NI U result(unsigned count,U x,unsigned unused) {
  (void)unused;
  struct A a={x,x+1,x+2}; struct B b={x+3}; struct C c={x+4}; struct P p={x+5};
  V4 v={1,2,3,(int)(x&255)}; V8 v8={1,2,3,4,5,6,7,(int)(x&255)};
  V16 v16={1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,(int)(x&255)};
  W w=((W)x<<64)|(x^0x123456789abcdefUL);
#define ARGS v,a,1.25,1UL,2UL,3UL,4UL,5UL,6UL,7UL, \
  0.5,1.5,2.5,3.5,4.5,5.5,6.5,7.5,8.5,w,b,2.125L,v8,77UL,c,v16,p,99
#define P0
#define P1 P0 1UL,0.25,
#define P2 P1 2UL,0.5,
#define P3 P2 3UL,0.75,
#define P4 P3 4UL,1.0,
#define P5 P4 5UL,1.25,
#define P6 P5 6UL,1.5,
#define P7 P6 7UL,1.75,
#define P8 P7 8UL,2.0,
#define P9 P8 9UL,2.25,
#define CALL(N) case N: return consume(b,N,P##N ARGS);
  switch(count) { CALL(0) CALL(1) CALL(2) CALL(3) CALL(4) CALL(5) CALL(6) CALL(7) CALL(8) CALL(9) }
  return 0;
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  const U inputs[]={0,1,7,0x7fffffffffffffffUL,0x8000000000000000UL,~0UL,
                    0x0123456789abcdefUL,0xfedcba9876543210UL};
  for(unsigned s=0;s<10;s++) for(unsigned i=0;i<8;i++) printf("%lx\n",result(s,inputs[i],0));
}
#endif
