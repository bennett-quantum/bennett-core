#!/usr/bin/env python3
"""Check coherent.py's recorded Grover amplitudes against closed forms, with no native run or matrix.

The marked set is the predicate read here by hand; after k iterations of D·O on the uniform state, with M of N
rows marked and sin θ = sqrt(M/N), each marked row holds sin((2k+1)θ)/sqrt(M) and each other row
cos((2k+1)θ)/sqrt(N-M) (M = 0 or N: the uniform state, multiplied by (-1)^k for M = N). Every recorded
amplitude, sign included, must equal its closed form, and no other configuration may hold weight.

    python3 experiments/lua-probe/grover_check.py target/lua-probe/oracle/coherent-history.json polynomial
"""
import argparse
import json
import math
from pathlib import Path
import re

TOLERANCE = 1e-12
PREDICATES = {
    # predicate.lua: ((n*n + 3*n + 1) % 7 == 4)
    'polynomial': lambda n: (n * n + 3 * n + 1) % 7 == 4,
    # predicates/three_of_eight.lua: n in {0, 3, 6}
    'three_of_eight': lambda n: n in (0, 3, 6),
}


def closed_form(bits, k):
    n, m = len(bits), sum(bits)
    if m in (0, n):
        sign = (-1) ** k if m == n else 1
        return [sign / math.sqrt(n)] * n
    theta = math.asin(math.sqrt(m / n))
    marked = math.sin((2 * k + 1) * theta) / math.sqrt(m)
    other = math.cos((2 * k + 1) * theta) / math.sqrt(n - m)
    return [marked if b else other for b in bits]


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('report', type=Path)
    ap.add_argument('predicate', choices=PREDICATES)
    args = ap.parse_args()
    rows = json.loads(args.report.read_text())
    checked = 0
    for row in rows:
        match = re.fullmatch(r'grover(\d+)_k(\d+)', row['name'])
        if not match:
            continue
        n, k = int(match[1]), int(match[2])
        bits = [int(PREDICATES[args.predicate](r)) for r in range(n)]
        assert row['native'] == bits, (row['name'], row['native'], bits)
        want = closed_form(bits, k)
        got = {tuple(a['config']): complex(a['real'], a['imag']) for a in row['amplitudes']}
        assert all(len(c) == 1 and c[0] < n for c in got), got
        error = max(abs(got.get((r,), 0) - want[r]) for r in range(n))
        assert error < TOLERANCE, (row['name'], error)
        success = sum(a * a for a, b in zip(want, bits) if b)
        print(f"{row['name']}: marked {[r for r in range(n) if bits[r]]}, success {success:.9f}, error {error:.2e}")
        checked += 1
    assert checked == 8, checked


if __name__ == '__main__':
    main()
