local ok, value = pcall(function() error("caught", 0) end)
print(ok, value)
print(pcall(function() return 42 end))
print(xpcall(function() error("nested", 0) end,
  function(e) return "handled:" .. e end))
