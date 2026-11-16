/* Embedding adapter only; every Lua translation unit is upstream source. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"
#include "scripts.h"

static int selected;

void lua_probe_select(int index) { selected = index; }

int main(void) {
    lua_State *state = luaL_newstate();
    if (!state) return 90;
    luaL_openlibs(state);
    int status = luaL_loadstring(state, probe_scripts[selected]);
    if (status == LUA_OK) status = lua_pcall(state, 0, 0, 0);
    if (status != LUA_OK) printf("error: %s\n", lua_tostring(state, -1));
    lua_close(state);
    return status;
}
