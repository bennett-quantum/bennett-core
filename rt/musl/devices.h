#ifndef BENNETT_DEVICES_H
#define BENNETT_DEVICES_H
#include <stdint.h>
void __bennett_effect(char);
/* cpu: a CPU-time clock, which does not count the time the process sleeps. */
uint64_t __bennett_clock_now(int cpu);
uint64_t __bennett_clock_read(int cpu);
void __bennett_clock_sleep(uint64_t);
void __bennett_entropy_check(uint64_t);
void __bennett_entropy_read(void *, uint64_t);
void __bennett_input_files(void);
long __bennett_device_syscall(long,long,long,long,long,long,long);
#endif
