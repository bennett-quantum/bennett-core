#ifdef NATIVE
#include <stdio.h>
#define emit(c) ((void)putchar(c))
#else
#include "bennett.h"
#define emit(c) __bennett_effect(c)
#endif
volatile int seed = 5;
int x;
/* Source order deliberately opposes priority order. */
__attribute__((constructor(202))) static void second(void) { x = x * 10 + 7; emit('2'); }
__attribute__((constructor(101))) static void first(void) { x = seed; emit('1'); }
__attribute__((destructor(101))) static void last(void) { emit('d'); }
int main(void) { emit('m'); return x; }
