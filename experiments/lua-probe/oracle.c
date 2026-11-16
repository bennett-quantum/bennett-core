/* Classical embedding; all parsing and evaluation is done by upstream Lua. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"
#include "predicate.h"

static lua_State *state;

int lua_oracle_setup(void) {
    state = luaL_newstate();
    if (!state) __builtin_trap();
    luaL_openlibs(state);
    if (luaL_loadstring(state, predicate_source) != LUA_OK ||
        lua_pcall(state, 0, 0, 0) != LUA_OK) __builtin_trap();
    return 0;
}

int lua_oracle_predicate(int input) {
    lua_getglobal(state, "predicate");
    lua_pushinteger(state, input);
    if (lua_pcall(state, 1, 1, 0) != LUA_OK) __builtin_trap();
    int result = (int)lua_tointeger(state, -1);
    lua_pop(state, 1);
    return result;
}

int main(void) {
    lua_oracle_setup();
    for (int i = 0; i < 8; ++i) printf("%d %d\n", i, lua_oracle_predicate(i));
    lua_close(state);
    return 0;
}
