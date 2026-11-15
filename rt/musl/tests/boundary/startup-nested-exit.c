#define _GNU_SOURCE
#include <unistd.h>
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <sys/syscall.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/uio.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#define RAW(n,a,b,c,d,e,f) syscall(SYS_##n,(long)(a),(long)(b),(long)(c),(long)(d),(long)(e),(long)(f))
#define CHECK(e) do { if (!(e)) return __LINE__; } while (0)
static void a(void) { fputs("a",stdout); }
static void b(void) { fputs("b",stdout); }
__attribute__((constructor(101))) static void c1(void) { fputs("1",stdout); }
__attribute__((constructor(201))) static void c2(void) { fputs("2",stdout); }
__attribute__((constructor)) static void c3(void) { fputs("3",stdout); }
__attribute__((destructor(101))) static void d1(void) { fputs("4",stdout); }
__attribute__((destructor(201))) static void d2(void) { fputs("5",stdout); }
__attribute__((destructor)) static void d3(void) { fputs("6",stdout); }
static void deep(void) { exit(37); }
int main(void) { atexit(a); atexit(b); fputs("M",stdout); deep(); return 99; }
