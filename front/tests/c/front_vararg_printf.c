/* Consume aligned objects, then pass the live and copied cursors into musl. */
#include <stdarg.h>
#include <stdio.h>
struct A { unsigned long x; } __attribute__((aligned(32)));
struct B { unsigned long x; } __attribute__((aligned(128)));
typedef int V __attribute__((vector_size(32)));
__attribute__((noinline)) int relay(int n,...) {
  va_list ap,again;
  va_start(ap,n);
  for(int i=0;i<n;i++) if(va_arg(ap,unsigned long)!=(unsigned long)(i+1)) return 1;
  struct A a=va_arg(ap,struct A);
  V v=va_arg(ap,V);
  struct B b=va_arg(ap,struct B);
  const char *fmt=va_arg(ap,const char *);
  if(a.x!=91 || b.x!=93 || v[3]!=94) return 2;
  va_copy(again,ap);
  char buf[256];
  int r=vsnprintf(buf,sizeof buf,fmt,again);
  va_end(again);
  int s=vprintf(fmt,ap);
  va_end(ap);
  puts(buf);
  return r!=s;
}
int main(void) {
  struct A a={91}; struct B b={93}; V v={1,2,3,94,5,6,7,8};
  const char *f="%d %ld %d %ld %d %ld %d %ld %.3f %.3Lf %s\n";
#define ARGS a,v,b,f,11,12L,13,14L,15,16L,17,18L,2.25,3.125L,"done"
  int r=relay(0,ARGS);
  return r ? r : relay(7,1UL,2UL,3UL,4UL,5UL,6UL,7UL,ARGS);
}
