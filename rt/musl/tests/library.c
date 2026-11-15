#include <math.h>
#include <stdio.h>
#include <stdlib.h>
volatile double input = 0.5;
volatile int seed = 5;
static int x;
__attribute__((constructor(202))) static void second(void) { x = x * 10 + 7; puts("ctor2"); }
__attribute__((constructor(101))) static void first(void) { x = seed; puts("ctor1"); }
__attribute__((destructor)) static void fini(void) { puts("dtor"); }
static void bye(void) { puts("atexit"); }
int main(void) {
    char *end;
    double parsed = strtod("1.25tail", &end);
    printf("%d %.6f %.6f %.6f %.6f %s\n", x, parsed, sqrt(input * 8), sin(input), pow(input * 4, 7), end);
    atexit(bye);
    return 3;
}
