/* The run's arguments as link.py writes them into the run-input file (inputs.c): their count, argv[0]
 * included, and their bytes, each string NUL-terminated, one after another, in an area whose size is a link
 * parameter (--argument-bytes). start.c builds argv from them. Run input, not start-up code: both are
 * writable, so no optimiser may take their values into code, and a relink with other arguments of the same
 * capacity changes the image's data only. */
#ifndef BENNETT_START_H
#define BENNETT_START_H
#include <stddef.h>
extern size_t __bennett_argument_count;
extern char __bennett_argument_bytes[];
#endif
