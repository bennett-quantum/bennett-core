local function counter(n)
  return function(delta) n = n + delta; return n end
end
local a, b = counter(10), counter(20)
print(a(2), a(3), b(-1), a(0))
