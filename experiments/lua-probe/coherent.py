#!/usr/bin/env python3
"""Coherent Lua predicate and Grover, with independent native/dense answers."""
import argparse
import json
import math
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
TOLERANCE = 1e-12


def wrapper(width):
    return f'''
func @lua_lookup{width}(i{width} %input) -> i1 {{
entry:
 %n = zext i{width} %input to i32
 %value = call @lua_oracle_predicate(%n)
 %bit = trunc i32 %value to i1
 ret %bit
}}
'''


def entry(name, width, basis=None, dirty=False, twice=False, iterations=None):
    lines = [f'func @{name}() -> i32 {{', 'entry:', ' %s = call @lua_oracle_setup()',
             f' %input = qalloc {width}', ' %out = qalloc 1', ' %fault = qalloc 5']
    lines += [f' h %input[{b}]' if basis is None else f' x %input[{b}]'
              for b in range(width) if basis is None or basis & (1 << b)]
    apply = f' apply @lua_lookup{width}(%input) -> %out, %fault budget 1000000'
    if iterations is None:
        if dirty:
            lines += [' x %out']
        lines += [apply]
        if twice:
            lines += [apply] + [f' h %input[{b}]' for b in reversed(range(width))]
            lines += [' qfree %fault', ' qfree %out', ' qfree %input']
    else:
        lines += [' x %out', ' h %out']
        for _ in range(iterations):
            lines += [apply]
            lines += [f' h %input[{b}]' for b in range(width)]
            lines += [f' x %input[{b}]' for b in range(width)]
            controls = ', '.join(f'%input[{b}]' for b in range(width - 1))
            lines += [f' z %input[{width - 1}] ctrl {controls}']
            lines += [f' x %input[{b}]' for b in reversed(range(width))]
            lines += [f' h %input[{b}]' for b in reversed(range(width))]
            # X on |-> corrects the sign of H X MCZ X H, so this is D,
            # including global phase, rather than -D.
            lines += [' x %out']
        lines += [' h %out', ' x %out', ' qfree %fault', ' qfree %out']
    return '\n'.join(lines + [' ret 0', '}']) + '\n'


def dense(bits, iterations):
    """Independent explicit matrix-vector multiplication, no circuit routines."""
    n = len(bits)
    amplitudes = [1 / math.sqrt(n)] * n
    oracle = [[float(i == j) * (-1 if bits[i] else 1) for j in range(n)] for i in range(n)]
    diffusion = [[2 / n - float(i == j) for j in range(n)] for i in range(n)]
    for _ in range(iterations):
        for matrix in (oracle, diffusion):
            amplitudes = [sum(a * b for a, b in zip(row, amplitudes)) for row in matrix]
    return amplitudes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('build', type=Path)
    args = parser.parse_args()
    build = args.build.resolve()
    mode = os.environ.get('BENNETT_APPLY', 'history')
    assert mode in ('history', 'branches', 'compare'), mode
    native = subprocess.run([str(build / 'native')], check=True, capture_output=True, text=True)
    pairs = [list(map(int, line.split())) for line in native.stdout.splitlines()]
    assert [p[0] for p in pairs] == list(range(8))
    bits = [p[1] for p in pairs]
    assert set(bits) == {0, 1}
    specs = [(f'basis{r}', dict(width=2, basis=r)) for r in range(4)]
    specs += [('uniform0', dict(width=2)), ('uniform1', dict(width=2, dirty=True)),
              ('interference', dict(width=2, twice=True))]
    specs += [(f'grover{1 << w}_k{k}', dict(width=w, iterations=k)) for w in (2, 3) for k in (0, 1, 2, 3)]
    text = (build / 'program.bn').read_text() + wrapper(2) + wrapper(3)
    text += ''.join(entry(name, **spec) for name, spec in specs)
    path = build / 'coherent.bn'
    path.write_text(text)
    result = subprocess.run([str(ROOT / 'target/debug/lua-probe-harness'), str(path),
                             ','.join(name for name, _ in specs)], capture_output=True, text=True)
    (build / f'coherent-{mode}-report.txt').write_text(result.stdout + result.stderr)
    assert result.returncode == 0, result.stdout + result.stderr
    reports = dict(re.findall(r'entry (\w+)\n(.*?)(?=\nentry |\Z)', result.stdout, re.S))
    rows = []
    for name, spec in specs:
        report = reports[name]
        actual = {}
        for config, real, imag in re.findall(r'basis \[([^\]]*)\] amplitude (\S+) (\S+)', report):
            config = tuple(int(v) for v in config.split(', ') if v)
            assert config not in actual
            actual[config] = complex(float(real), float(imag))
        n = 1 << spec['width']
        if 'iterations' in spec:
            expected = {(r,): complex(a) for r, a in enumerate(dense(bits[:n], spec['iterations']))}
        elif spec.get('twice'):
            expected = {(): 1 + 0j}
        else:
            inputs = [spec['basis']] if 'basis' in spec else range(n)
            amplitude = 1 if 'basis' in spec else 1 / math.sqrt(n)
            expected = {(r, bits[r] ^ int(spec.get('dirty', False)), 0): complex(amplitude) for r in inputs}
        error = max(abs(actual.get(c, 0j) - expected.get(c, 0j)) for c in actual.keys() | expected.keys())
        assert error < TOLERANCE, (name, actual, expected, error)
        assert abs(sum(abs(a) ** 2 for a in actual.values()) - 1) < TOLERANCE
        assert 'reverse identical' in report
        row = dict(name=name, native=bits[:n], max_amplitude_error=error,
                   amplitudes=[dict(config=list(c), real=a.real, imag=a.imag) for c, a in sorted(actual.items())],
                   oracles=re.findall(r'^oracle .+$', report, re.M), report=report)
        if 'iterations' in spec:
            row['success'] = sum(abs(actual.get((r,), 0j)) ** 2 for r in range(n) if bits[r])
        rows.append(row)
        print(json.dumps({k: v for k, v in row.items() if k != 'report'}), flush=True)
    (build / f'coherent-{mode}.json').write_text(json.dumps(rows, indent=2) + '\n')


if __name__ == '__main__':
    main()
