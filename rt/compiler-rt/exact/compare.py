"""compare.py A B [random]: the output of cplx_exact.c from two builds; per helper, the tuples whose results
differ only in the bits of NaN results (sign, payload), and those that differ otherwise (the first few shown)."""
import sys
ND, R = 16, int(sys.argv[3]) if len(sys.argv) > 3 else 16384
N = ND**4 + R
ops = [("mulsc3", 8), ("divsc3", 8), ("muldc3", 16), ("divdc3", 16), ("mulxc3", 20), ("divxc3", 20)]
def nan(h, w):
    v = int(h, 16)
    if w == 8: return (v & 0x7fffffff) > 0x7f800000
    if w == 16: return (v & 0x7fffffffffffffff) > 0x7ff0000000000000
    return (v >> 64) & 0x7fff == 0x7fff and (v & ((1 << 63) - 1)) != 0
a, b = (open(p).read().strip() for p in sys.argv[1:3])
pos = 0
for name, w in ops:
    size = N * 2 * w
    if pos >= len(a):
        break
    x, y = a[pos:pos + size], b[pos:pos + size]
    pos += size
    exact = payload = other = 0
    shown = 0
    for i in range(N):
        ra, ia = x[2*i*w:(2*i+1)*w], x[(2*i+1)*w:(2*i+2)*w]
        rb, ib = y[2*i*w:(2*i+1)*w], y[(2*i+1)*w:(2*i+2)*w]
        if (ra, ia) == (rb, ib):
            exact += 1
        elif all(p == q or (nan(p, w) and nan(q, w)) for p, q in ((ra, rb), (ia, ib))):
            payload += 1
        else:
            other += 1
            if shown < 3:
                shown += 1
                print(f"  {name} #{i} ({'directed' if i < ND**4 else 'random'}): {ra} {ia} vs {rb} {ib}")
    print(f"{name}: {N} tuples, {exact} identical, {payload} differ only in NaN bits, {other} differ otherwise")
assert pos == len(a) == len(b), (pos, len(a), len(b))
