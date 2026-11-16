local t = {7, 2, 11, 3}
table.sort(t)
t.answer = t[1] + t[4]
print(table.concat(t, ","), t.answer, #t)
