#include <stdio.h>
#include <stdlib.h>
static void a(void) { fputs("A",stdout); }
static void b(void) { fputs("B",stdout); }
__attribute__((destructor)) static void d(void) { fputs("D",stdout); }
int main(void) { for(int i=0;i<40;i++) if(atexit(i%2?a:b)) return 1; fputs("M",stdout); return 17; }
