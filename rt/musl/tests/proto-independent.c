/* Independent native oracle: libc results, fixed calls of variadics and qsort's indirect callbacks. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U;

__attribute__((noinline)) static int compare_addresses(U a,U b) {
  int x=*(const int *)a,y=*(const int *)b;
  return (x>y)-(x<y);
}
__attribute__((noinline)) static int compare_extra(const void *a,const void *b,U unread) {
  (void)unread;
  int x=*(const int *)a,y=*(const int *)b;
  return (x>y)-(x<y);
}
static int (*volatile comparisons[2])(const void *,const void *)={
  (int (*)(const void *,const void *))compare_addresses,
  (int (*)(const void *,const void *))compare_extra
};

int main(void) {
  U (*volatile division)(int,int)=(U (*)(int,int))div;
  long (*volatile quotient)(long,long)=(long (*)(long,long))ldiv;
  int (*volatile parse)(const char *,char **,int,U)=(int (*)(const char *,char **,int,U))strtol;
  int (*volatile format)(char *,U,const char *,U,U)=(int (*)(char *,U,const char *,U,U))snprintf;
  int (*volatile print)(const char *,U,U)=(int (*)(const char *,U,U))printf;
  char buffer[64],*end;
  U d=division(71,9);
  if(d!=((U)7|((U)8<<32))||quotient(-71,9)!=-7)return 1;
  int parsed=parse("12345!",&end,10,99);
  if(parsed!=12345||*end!='!')return 2;
  if(format(buffer,sizeof buffer,"%lu/%lu",(U)17,(U)23)!=5||strcmp(buffer,"17/23"))return 3;
  for(unsigned mode=0;mode<2;mode++) {
    int values[]={31,-7,23,0,19,-3,41,2};
    qsort(values,8,sizeof values[0],comparisons[mode]);
    for(unsigned i=1;i<8;i++)if(values[i-1]>values[i])return 4;
    printf("%u:",mode);
    for(unsigned i=0;i<8;i++)printf(" %d",values[i]);
    puts("");
  }
  print("%lu %lu\n",d,(U)parsed);
  puts(buffer);
  return 0;
}
