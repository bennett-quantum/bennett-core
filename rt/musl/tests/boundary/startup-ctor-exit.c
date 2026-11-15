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
static void bye(void) { fputs("A",stdout); }
__attribute__((destructor)) static void d(void) { fputs("D",stdout); }
__attribute__((constructor(101))) static void c(void) { atexit(bye); fputs("C",stdout); exit(23); }
__attribute__((constructor(201))) static void late(void) { fputs("L",stdout); }
int main(void) { fputs("M",stdout); return 99; }
