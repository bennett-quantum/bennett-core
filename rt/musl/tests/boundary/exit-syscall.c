#define _GNU_SOURCE
#include <sys/syscall.h>
#include <unistd.h>
#ifndef CALL
#define CALL SYS_exit
#endif
int main(void) { syscall(CALL, 43L, 0L, 0L, 0L, 0L, 0L); return 99; }
