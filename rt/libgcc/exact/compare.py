"""compare.py A B [random]: the output of cplx_exact.c from two builds; per helper, the tuples whose results
differ only in the bits of NaN results (sign, payload), and those that differ otherwise (the first few shown)."""
import sys
ND, R = 20, int(sys.argv[3]) if len(sys.argv) > 3 else 262144
N = ND**4 + R
ops = [("mulsc3", 8), ("divsc3", 8), ("muldc3", 16), ("divdc3", 16), ("mulxc3", 20), ("divxc3", 20)]
def nan(h, w):
    v = int(h, 16)
    if w == 8: return (v & 0x7fffffff) > 0x7f800000
    if w == 16: return (v & 0x7fffffffffffffff) > 0x7ff0000000000000
    return (v >> 64) & 0x7fff == 0x7fff and (v & ((1 << 63) - 1)) != 0
a, b = (open(p).read().strip() for p in sys.argv[1:3])
assert len(a) == len(b) == N * sum(2*w for _, w in ops), (len(a), len(b), N)
pos = 0
failures = 0
for name, w in ops:
    size = N * 2 * w
    x, y = a[pos:pos + size], b[pos:pos + size]
    pos += size
    exact = payload = other = 0
    shown = nan_shown = 0
    for i in range(N):
        ra, ia = x[2*i*w:(2*i+1)*w], x[(2*i+1)*w:(2*i+2)*w]
        rb, ib = y[2*i*w:(2*i+1)*w], y[(2*i+1)*w:(2*i+2)*w]
        if (ra, ia) == (rb, ib):
            exact += 1
        elif all(p == q or (nan(p, w) and nan(q, w)) for p, q in ((ra, rb), (ia, ib))):
            payload += 1
            if nan_shown < 2:
                nan_shown += 1
                print(f"  {name} #{i} NaN bits: {ra} {ia} vs {rb} {ib}")
        else:
            other += 1
            if shown < 3:
                shown += 1
                print(f"  {name} #{i} ({'directed' if i < ND**4 else 'random'}): {ra} {ia} vs {rb} {ib}")
    failures += payload + other
    print(f"{name}: {N} tuples, {exact} identical, {payload} differ only in NaN bits, {other} differ otherwise")
assert pos == len(a) == len(b), (pos, len(a), len(b))

# Exact means raw bits, including NaNs. The fast grouped suite tests the weaker FP contract.
sys.exit(bool(failures))
