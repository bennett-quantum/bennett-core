/* ISO C reserves no name timer_create: a program may define its own, and the C library's then stays out
 * (rt/musl/timer.c is an archive member, pulled only when a program calls timer_create without defining it). */
#include <stdio.h>

int timer_create(void)
{
    return 7;
}

int main(void)
{
    printf("own timer_create: %d\n", timer_create());
    return 0;
}
