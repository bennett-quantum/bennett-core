local co = coroutine.create(function(n)
  for i = 1, n do coroutine.yield(i * i) end
  return "done"
end)
for i = 1, 4 do print(coroutine.resume(co, 3)) end
print(coroutine.status(co))
