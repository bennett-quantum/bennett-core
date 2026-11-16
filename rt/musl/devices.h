#ifndef BENNETT_DEVICES_H
#define BENNETT_DEVICES_H
#include <stdint.h>
void __bennett_effect(char);
uint64_t __bennett_clock_read(void);
void __bennett_entropy_check(uint64_t);
void __bennett_entropy_read(void *, uint64_t);
void __bennett_input_files(void);
long __bennett_device_syscall(long,long,long,long,long,long,long);
#endif
