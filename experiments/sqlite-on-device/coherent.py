#!/usr/bin/env python3
"""Append only quantum glue to the lowered N=4 program and inspect all amplitudes.

No SQLite instruction is replaced. Expected bits come from the native driver.
Build the standalone harness first (README). Raw artifacts stay in BUILD.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def entry(name, row=None, out=0, budget=100000, twice=False):
    lines = [f'func @{name}() -> i32 {{', 'entry:',
             ' %s = call @experiment_setup()', ' %row = qalloc 2',
             ' %out = qalloc 1', ' %fault = qalloc 5']
    for bit in range(2):
        if row is None:
            lines.append(f' h %row[{bit}]')
        elif row & (1 << bit):
            lines.append(f' x %row[{bit}]')
    if out:
        lines.append(' x %out')
    apply = f' apply @sqlite_lookup(%row) -> %out, %fault budget {budget}'
    lines.append(apply)
    if twice:
        lines += [apply, ' h %row[1]', ' h %row[0]',
                  ' qfree %fault', ' qfree %out', ' qfree %row']
    lines += [' ret 0', '}']
    return '\n'.join(lines)+'\n'


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('build', type=Path)
    args = ap.parse_args()
    build = args.build.resolve()
    native = subprocess.run([str(build/'native')], capture_output=True, text=True, check=True)
    answers = [int(v) for v in re.findall(r'^like_note \d+ (\d+)$', native.stdout, re.M)]
    assert len(answers) == 4 and set(answers) == {0, 1}, answers
    wrapper = '''
func @sqlite_lookup(i2 %row) -> i1 {
entry:
 %r = zext i2 %row to i32
 %v = call @experiment_query(i32 2, %r)
 %b = trunc i32 %v to i1
 ret %b
}
'''
    names = [f'basis{r}' for r in range(4)] + ['uniform0', 'uniform1', 'interference', 'budget_probe', 'min_budget', 'boundary_probe']
    text = (build/'program.bn').read_text()+wrapper
    for r in range(4):
        text += entry(f'basis{r}', row=r)
    text += entry('uniform0')+entry('uniform1', out=1)
    text += entry('interference', twice=True)+entry('budget_probe', budget=1000)
    text += entry('min_budget', budget=11048)+entry('boundary_probe', budget=11047)
    program = build/'coherent.bn'
    program.write_text(text)
    harness = ROOT/'target/debug/sqlite-on-device-harness'
    p = subprocess.run([str(harness), str(program), ','.join(names)], capture_output=True, text=True)
    (build/'coherent-report.txt').write_text(p.stdout+p.stderr)
    assert p.returncode == 0, p.stdout+p.stderr
    observed = {}
    for name, block in re.findall(r'entry (\w+)\n(.*?)(?=\nentry |\Z)', p.stdout, re.S):
        assert 'outcome Returned(Some(0))' in block, block
        basis = []
        for config, real, imag in re.findall(r'basis \[([^\]]*)\] amplitude (\S+) (\S+)', block):
            basis.append(([int(v) for v in config.split(', ') if v], float(real), float(imag)))
        if name.startswith('basis'):
            r = int(name[5:])
            expected = [([r, answers[r], 0], 1.0, 0.0)]
        elif name in ['uniform0', 'uniform1', 'min_budget']:
            out = int(name[-1]) if name.startswith('uniform') else 0
            expected = [([r, out ^ answers[r], 0], 0.5, 0.0) for r in range(4)]
        elif name == 'interference':
            # Freed columns disappear from the configuration.
            expected = [([], 1.0, 0.0)]
        elif name == 'boundary_probe':
            expected = [([r, 0, int(r == 3)], 0.5, 0.0) for r in range(4)]
        else:
            expected = [([r, 0, 1], 0.5, 0.0) for r in range(4)]  # Fault::Budget = 1
        assert len(basis) == len(expected), (name, basis, expected)
        for actual, want in zip(basis, expected):
            assert actual[0] == want[0], (name, basis, expected)
            assert abs(actual[1]-want[1]) < 1e-12 and abs(actual[2]-want[2]) < 1e-12
        observed[name] = dict(basis=basis, report=block)
    assert len(observed) == len(names)
    # This also checks the entire classical setup and the coherent operation un-run exactly.
    p = subprocess.run([str(harness), str(program), 'uniform0', 'reverse'], capture_output=True, text=True)
    (build/'coherent-reverse.txt').write_text(p.stdout+p.stderr)
    assert p.returncode == 0 and 'reverse identical' in p.stdout, p.stdout+p.stderr
    observed['reverse'] = 'identical'
    (build/'coherent.json').write_text(json.dumps(observed, indent=2)+'\n')
    print(json.dumps(observed, indent=2))


if __name__ == '__main__':
    main()
