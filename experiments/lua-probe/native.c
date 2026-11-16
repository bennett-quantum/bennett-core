#include <stdlib.h>
void lua_probe_select(int index);
int lua_device_main(void);
int main(int argc, char **argv) {
    if (argc != 2) return 91;
    lua_probe_select(atoi(argv[1]));
    return lua_device_main();
}
