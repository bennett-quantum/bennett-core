#include "bennett.h"
int lua_oracle_setup(void);
int lua_oracle_predicate(int input);

unsigned long long lua_coherent_c(void) {
    lua_oracle_setup();
    unsigned char input = 0;
    _Bool output = 0;
    bennett_h(bennett_bit(&input, 0));
    bennett_h(bennett_bit(&input, 1));
    output ^= lua_oracle_predicate(input);
    return bennett_measure(&output);
}
