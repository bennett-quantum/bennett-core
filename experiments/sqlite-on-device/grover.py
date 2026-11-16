#!/usr/bin/env python3
"""Grover using the unchanged lowered SQLite LIKE query; check every amplitude.

Build the ordinary n4/n8 module and standalone observer first (README).
SQLITE_DIR supplies the original amalgamation for an independent native check.
All generated sources, reports and measurements go to BUILD.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
TOLERANCE = 1e-12


def update_sql(marked, n):
    if n == 4 and marked == [3]:
        return ''  # The original stage-3 database already marks row index 3.
    # Preserve generated text and its length, remove preexisting matches, and
    # append the marker only to selected rows. xx cannot create a new zq.
    sql = "UPDATE t SET note=replace(lower(note),'zq','xx');"
    if marked:
        ids = ','.join(str(r + 1) for r in marked)
        sql += f"UPDATE t SET note=note||'zq' WHERE id IN ({ids});"
    return sql


def diffusion(width):
    lines = [f' h %row[{b}]' for b in range(width)]
    lines += [f' x %row[{b}]' for b in range(width)]
    controls = ', '.join(f'%row[{b}]' for b in range(width - 1))
    lines += [f' z %row[{width - 1}] ctrl {controls}']
    lines += [f' x %row[{b}]' for b in reversed(range(width))]
    lines += [f' h %row[{b}]' for b in reversed(range(width))]
    # H X MCZ X H = I - 2|s><s|. X on |-> supplies the minus sign,
    # giving D exactly, including its global phase for amplitude comparison.
    return lines + [' x %out']


def program(base, width, cases):
    memory = int(re.search(r'^memory (\d+)$', base, re.M)[1])
    # Add SQL constants above the entire original memory, retaining its full
    # static image/functions verbatim; reserve a fresh 1 MiB stack above them.
    cursor = memory
    extra = []
    db = re.search(r'call @sqlite3_open\(i64 \d+, i64 (\d+)\)', base)[1]
    extra += [f'''func @sqlite_grover_lookup(i{width} %row) -> i1 {{
entry:
 %r = zext i{width} %row to i32
 %v = call @experiment_query(i32 2, %r)
 %b = trunc i32 %v to i1
 ret %b
}}''']
    for name, marked, iterations in cases:
        sql = update_sql(marked, 1 << width)
        encoded = ''.join(f'\\x{b:02x}' for b in sql.encode() + b'\0')
        extra += [f'data {cursor} = "{encoded}"', f'''func @{name}_setup() -> i32 {{
entry:
 %s = call @experiment_setup()
 %db = load i64 {db}
 %rc = call @sqlite3_exec(%db, i64 {cursor}, i64 0, i64 0, i64 0)
 call @sod_check(%rc)
 ret 0
}}''']
        cursor += len(sql.encode()) + 1
        lines = [f'func @{name}() -> i32 {{', 'entry:',
                 f' %s = call @{name}_setup()', f' %row = qalloc {width}',
                 ' %out = qalloc 1', ' %fault = qalloc 5']
        lines += [f' h %row[{b}]' for b in range(width)]
        lines += [' x %out', ' h %out']
        for _ in range(iterations):
            lines += [' apply @sqlite_grover_lookup(%row) -> %out, %fault budget 100000']
            lines += diffusion(width)
        lines += [' h %out', ' x %out', ' qfree %fault', ' qfree %out',
                  ' %measured = measure %row',
                  f' %result = zext i{width} %measured to i32', ' ret %result', '}']
        extra += ['\n'.join(lines)]
    return re.sub(r'^memory \d+$', f'memory {cursor + 1048576}', base, count=1, flags=re.M) + '\n' + '\n'.join(extra) + '\n'


def native_driver(build, sqlite_dir, n):
    for filename, expected in {
        'sqlite3.c': '0c657666d819f261cc4007526ad57e1669de9949df7a8ea847aa284a2bf0ee52',
        'sqlite3.h': 'd6c84557261893439b341d2e6c35187ae3ea24fbfd843ad94616f72d89242b8c',
    }.items():
        assert hashlib.sha256((sqlite_dir / filename).read_bytes()).hexdigest() == expected
    source = build / 'grover-native.c'
    source.write_text('''#include "sqlite3.c"
#define main original_main
#include "driver.c"
#undef main
int main(int argc, char **argv) {
  if (argc != 2) return 3;
  experiment_setup();
  sod_check(sqlite3_exec(sod_db, argv[1], 0, 0, 0));
  for (int r=0; r<NROWS; ++r) printf("%d %d\\n", r, experiment_query(2,r));
  return 0;
}
''')
    exe = build / 'grover-native'
    subprocess.run(['clang-18', '-O2', '-fno-vectorize', '-fno-slp-vectorize',
                    '-DSQLITE_THREADSAFE=0', '-DSQLITE_OMIT_LOAD_EXTENSION',
                    '-DSQLITE_OS_OTHER=1', '-DSQLITE_ENABLE_MEMSYS5', '-DSQLITE_TEMP_STORE=3',
                    f'-DNROWS={n}', '-DHEAP_BYTES=262144', f'-I{sqlite_dir}',
                    f'-I{ROOT / "experiments/sqlite-on-device"}', str(source), '-o', str(exe)], check=True)
    return exe


def expected_state(n, marked, k):
    if not marked:
        return [1 / math.sqrt(n)] * n
    if n == 4:
        if len(marked) == 1 and k == 1:
            return [float(r in marked) for r in range(n)]
        if len(marked) == 2:
            good, bad = {1: (0.5, -0.5), 2: (-0.5, -0.5), 3: (-0.5, 0.5)}[k]
            return [good if r in marked else bad for r in range(n)]
    theta = math.asin(math.sqrt(len(marked) / n))
    good = math.sin((2 * k + 1) * theta) / math.sqrt(len(marked))
    bad = math.cos((2 * k + 1) * theta) / math.sqrt(n - len(marked))
    return [good if r in marked else bad for r in range(n)]


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('build', type=Path)
    ap.add_argument('--n', type=int, choices=[4, 8], required=True)
    ap.add_argument('--sqlite-dir', type=Path, default=os.environ.get('SQLITE_DIR'))
    args = ap.parse_args()
    assert args.sqlite_dir, 'set SQLITE_DIR or --sqlite-dir'
    build, n = args.build.resolve(), args.n
    width = n.bit_length() - 1
    if n == 4:
        cases = [(f'single{r}_k1', [r], 1) for r in range(n)]
        cases += [(f'pair_k{k}', [0, 3], k) for k in [1, 2, 3]]
        cases += [('none_k1', [], 1)]
    else:
        cases = [(f'single3_k{k}', [3], k) for k in [1, 2]]
        cases += [('pair_k1', [0, 3], 1)]
    native = native_driver(build, args.sqlite_dir.resolve(), n)
    native_answers = {}
    for name, marked, _ in cases:
        output = subprocess.run([str(native), update_sql(marked, n)], check=True, capture_output=True, text=True).stdout
        answers = [int(v) for _, v in re.findall(r'^(\d+) (\d+)$', output, re.M)]
        assert answers == [int(r in marked) for r in range(n)], (name, output)
        native_answers[name] = answers
    path = build / 'grover.bn'
    base = (build / 'program.bn').read_text()
    path.write_text(program(base, width, cases))
    harness = ROOT / 'target/debug/sqlite-on-device-harness'
    run = subprocess.run([str(harness), str(path), 'grover', ','.join(c[0] for c in cases)], capture_output=True, text=True)
    (build / 'grover-report.txt').write_text(run.stdout + run.stderr)
    assert run.returncode == 0, run.stdout + run.stderr
    reports = dict(re.findall(r'entry (\w+)\n(.*?)(?=\nentry |\Z)', run.stdout, re.S))
    result = []
    for name, marked, k in cases:
        report = reports[name]
        measured = [0j] * n
        for config, real, imag in re.findall(r'basis \[([^\]]*)\] amplitude (\S+) (\S+)', report):
            config = [int(v) for v in config.split(', ')]
            assert len(config) == 1 and 0 <= config[0] < n, (name, config)
            measured[config[0]] = complex(float(real), float(imag))
        expected = expected_state(n, marked, k)
        deviation = max(abs(a - b) for a, b in zip(measured, expected))
        assert deviation < TOLERANCE, (name, measured, expected, deviation)
        success = sum(abs(measured[r]) ** 2 for r in marked)
        theory = sum(expected[r] ** 2 for r in marked)
        assert abs(sum(abs(a) ** 2 for a in measured) - 1) < TOLERANCE
        sampled = int(re.search(r'outcome Returned\(Some\((\d+)\)\)', report)[1])
        assert abs(measured[sampled]) > TOLERANCE
        if abs(theory - 1) < TOLERANCE:
            assert sampled in marked
        oracle_lines = re.findall(r'^oracle .+$', report, re.M)
        assert len(oracle_lines) == k
        # Constant full support: per-call cost must be identical, rather than
        # just trusting cumulative counters. Collapsed sparse support can cost less.
        if (n == 4 and len(marked) == 2) or (n == 8 and len(marked) == 1):
            assert len({line.split(' region_steps ', 1)[1] for line in oracle_lines}) == 1
        item = dict(n=n, marked=marked, iterations=k, native=native_answers[name],
                    theory_success=theory, measured_success=success, max_amplitude_deviation=deviation,
                    expected_amplitudes=expected, measured_amplitudes=[[a.real, a.imag] for a in measured],
                    sampled_row=sampled, oracles=oracle_lines, report=report)
        result.append(item)
        print(json.dumps({key: value for key, value in item.items() if key != 'report'}), flush=True)
    (build / 'grover.json').write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
