-- Iteration only where Lua determines the order: numeric for, ipairs, sort with a total order, sequences.
local t = {}
for i = 1, 40 do t[i] = (i * 37) % 41 end
local out = {}
for i, v in ipairs(t) do out[#out + 1] = v end
print(table.concat(out, ","))
table.sort(t)
print(table.concat(t, " ", 1, 10), #t)
table.sort(t, function(a, b) return a > b end)
print(table.concat(t, " ", 1, 10))
local words = {"pear", "Apple", "fig", "apple", "Banana", "kiwi", "banana"}
table.sort(words)
print(table.concat(words, " "))
table.sort(words, function(a, b) return a:lower() < b:lower() or (a:lower() == b:lower() and a < b) end)
print(table.concat(words, " "))
local s = {1, 2, 3, 4, 5}
table.insert(s, 1, 0); table.insert(s, 9); print(table.remove(s), table.remove(s, 1), table.concat(s, ","))
print(table.unpack(table.move({1, 2, 3, 4, 5}, 2, 4, 1)))
print(table.concat(table.move({1, 2, 3}, 1, 3, 3, {9, 9}), ","))
print(select("#", nil, nil, nil), select(-1, 1, 2, 3), select(2, "a", "b", "c"))
print(#"", #{}, #{n = 1}, rawlen({1, 2}), rawequal(t, t), next({}))
local packed = table.pack(1, nil, 3)
print(packed.n, packed[1], packed[2], packed[3])
local holes = {}
for i = 1, 100 do holes[i] = i * i end
for i = 100, 51, -1 do holes[i] = nil end
print(#holes, holes[50], holes[51])
local deep = {}
for i = 1, 200 do deep = {deep, i} end
local depth = 0
while deep[1] do depth = depth + deep[2]; deep = deep[1] end
print(depth)
print(pcall(table.sort, {3, 1, "x"}))
print(pcall(table.insert, {1}, 5, 2))
print(string.rep("ab", 3, "-"), ("x"):rep(0), ("abc"):byte(1, -1))
print(utf8.char(72, 228, 8364, 128512), utf8.len("häll€"), #utf8.char(128512))
for p, c in utf8.codes("aé€") do io.write(p, ":", c, " ") end
print()
