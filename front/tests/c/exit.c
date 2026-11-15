#include "bennett.h"
volatile int limit = 8;
__attribute__((noinline)) static void nested(int status) {
    __bennett_effect('n');
    __bennett_exit(status);
}
int from_nested(int status) { nested(status); return 99; }
int from_loop(int status) {
    for (int i = 0; i < limit; ++i) {
        __bennett_effect('0' + i);
        if (i == 3) __bennett_exit(status);
    }
    return 99;
}
