#include "bennett.h"
__attribute__((noinline)) void emit(char c) { __bennett_effect(c); }
int main(void) {
    const char bytes[] = {'h', 'i', '\n', 0, (char)255};
    for (unsigned i = 0; i < sizeof bytes; ++i) emit(bytes[i]);
    return 7;
}
