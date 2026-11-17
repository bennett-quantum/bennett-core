-- Garbage collection under pressure: many short-lived tables and strings, weak tables, finalizer order
-- (the manual: finalizers run in the reverse order of marking), generational and incremental modes.
collectgarbage("collect")
local base = collectgarbage("count")
local keep = {}
for i = 1, 3000 do
  local t = {i, tostring(i), {i}}
  if i % 100 == 0 then keep[#keep + 1] = t end
end
collectgarbage("collect")
print(#keep, keep[30][2], collectgarbage("count") - base > 0)
local order = {}
for i = 1, 5 do setmetatable({}, {__gc = function() order[#order + 1] = i end}) end
collectgarbage("collect")
print(table.concat(order, ","))
local weak = setmetatable({}, {__mode = "k"})
local strong = {}
for i = 1, 10 do local k = {} weak[k] = i if i % 2 == 0 then strong[#strong + 1] = k end end
collectgarbage("collect")
local n = 0
for _ in pairs(weak) do n = n + 1 end
print(n)
local values = setmetatable({}, {__mode = "v"})
for i = 1, 10 do values[i] = {i} end
values[11] = "strings are values, not collected"
collectgarbage("collect")
local live = 0
for i = 1, 11 do if values[i] then live = live + 1 end end
print(live)
print(collectgarbage("generational"), collectgarbage("incremental"), collectgarbage("isrunning"))
local s = ""
for i = 1, 400 do s = s .. string.char(65 + i % 26) end
print(#s, s:sub(1, 30), collectgarbage("step", 0) ~= nil)
collectgarbage("stop")
print(collectgarbage("isrunning"))
collectgarbage("restart")
local resurrected
setmetatable({}, {__gc = function(o) resurrected = o end})
collectgarbage("collect")
print(type(resurrected))
local before = collectgarbage("count")
for i = 1, 2000 do local _ = string.rep("z", 64) .. i end
collectgarbage("collect")
print(collectgarbage("count") - before < 64)
