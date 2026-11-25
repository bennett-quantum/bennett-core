/* The run's arguments as link.py writes them into the run-input file (inputs.c): their count, argv[0]
 * included, and their bytes, each string NUL-terminated, one after another. start.c builds argv from them.
 * Run input, not start-up code. */
#ifndef BENNETT_START_H
#define BENNETT_START_H
#include <stddef.h>
extern const size_t __bennett_argument_count;
extern char __bennett_argument_bytes[];
#endif
