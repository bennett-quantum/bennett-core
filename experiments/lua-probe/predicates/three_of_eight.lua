-- Three of eight marked, by a different path through the VM: string, table and closure.
local marks = {}
for _, n in ipairs({0, 3, 6}) do marks[tostring(n)] = true end
function predicate(n)
  local check = function(k) return marks[string.format("%d", k)] end
  return check(n) and 1 or 0
end
