#!/usr/bin/env python3
"""Recompute the SQL oracle's marked sets and Grover amplitudes without the native build.

The generated table is rebuilt from ../driver.c's LCG and text generator; each query is answered by the
host Python's own SQLite and by a plain-Python reading of it, which must agree. Given a scale build
directory, the native driver's printed truth table must equal both; with --amplitudes, each Grover entry
that ../scale.py wrote is run to its measurement by the observer's `dump` mode and every amplitude is
compared with a dense Grover iteration written here.

    python3 experiments/sqlite-on-device/edge/truth.py target/sqlite-on-device/oracle16 --n 16 --amplitudes
"""
import argparse
import math
from pathlib import Path
import re
import sqlite3
import subprocess

ROOT = Path(__file__).resolve().parents[3]
M64 = (1 << 64) - 1
TOLERANCE = 1e-12
# (SQL, offset added to the row index for its parameter, plain reading of the query).
SHAPES = {
    'arith': ('SELECT (a*7+b)%13=5 FROM t WHERE id=?', 1, lambda r, rows: (r[1] * 7 + r[2]) % 13 == 5),
    'like_name': ("SELECT name LIKE '%qu%' FROM t WHERE id=?", 1, lambda r, rows: 'qu' in r[3].lower()),
    'like_note': ("SELECT note LIKE '%zq%' FROM t WHERE id=?", 1, lambda r, rows: 'zq' in r[4].lower()),
    'scan': ('SELECT count(*) FROM t WHERE a=?', 0, None),
    'int_eq': ('SELECT a=3 FROM t WHERE id=?', 1, lambda r, rows: r[1] == 3),
    'text_order': ("SELECT name<'m' FROM t WHERE id=?", 1, lambda r, rows: r[3].encode() < b'm'),
    'two_where': ('SELECT EXISTS(SELECT 1 FROM t WHERE id=? AND a<8 AND b>=500)', 1,
                  lambda r, rows: r[1] < 8 and r[2] >= 500),
    'range_agg': ('SELECT count(*)>0 FROM t WHERE a BETWEEN ? AND ?+1', 0, None),
}
ORACLES = ['like_note', 'int_eq', 'text_order', 'two_where', 'range_agg']


class Lcg:
    def __init__(self):
        self.state = 0x2545F4914F6CDD1D

    def next(self):
        self.state = (self.state * 6364136223846793005 + 1442695040888963407) & M64
        return self.state >> 33

    def text(self, n):
        out = []
        for i in range(n):
            # C's && short-circuits: a second draw only when the first does not make a space.
            if self.next() % 6 == 0 and i and out[i - 1] != ' ':
                out.append(' ')
            else:
                out.append(chr(ord('a') + self.next() % 26))
        return ''.join(out)


def rows(n):
    g, result = Lcg(), []
    for row_id in range(1, n + 1):
        name_length = 4 + g.next() % 37
        long_note = g.next() % 32 == 0
        note_length = 4000 + g.next() % 8000 if long_note else 20 + g.next() % 380
        name, note = g.text(name_length), g.text(note_length)
        a = g.next() % n
        b = g.next() % 1000
        result.append((row_id, a, b, name, note))
    return result


def truth(n):
    table = rows(n)
    db = sqlite3.connect(':memory:')
    db.execute('CREATE TABLE t(id INTEGER PRIMARY KEY,a INTEGER,b INTEGER,name TEXT,note TEXT)')
    db.executemany('INSERT INTO t VALUES(?,?,?,?,?)', table)
    answers = {}
    for shape, (sql, offset, reading) in SHAPES.items():
        values = []
        for r in range(n):
            arg = r + offset
            host = db.execute(sql, (arg,) * sql.count('?')).fetchone()[0]
            if shape == 'scan':
                plain = sum(1 for x in table if x[1] == arg)
            elif shape == 'range_agg':
                plain = int(any(arg <= x[1] <= arg + 1 for x in table))
            else:
                plain = int(reading(table[arg - 1], table))
            assert host == plain, (shape, n, r, host, plain)
            values.append(host)
        answers[shape] = values
    return answers


def grover(bits):
    n, marked = len(bits), sum(bits)
    if 0 < marked < n:
        k = max(1, round(math.pi / (4 * math.asin(math.sqrt(marked / n))) - 0.5))
    else:
        k = 1
    psi = [1 / math.sqrt(n)] * n
    for _ in range(k):
        psi = [-a if b else a for a, b in zip(psi, bits)]
        mean = sum(psi) / n
        psi = [2 * mean - a for a in psi]
    return k, psi


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('build', type=Path, nargs='?')
    ap.add_argument('--n', type=int, required=True)
    ap.add_argument('--amplitudes', action='store_true')
    ap.add_argument('--harness', type=Path, default=ROOT/'target/release/sqlite-on-device-harness')
    args = ap.parse_args()
    answers = truth(args.n)
    if args.build:
        native = subprocess.run([args.build/'native'], capture_output=True, text=True, check=True).stdout
        for shape, values in answers.items():
            printed = [int(v) for v in re.findall(rf'^{shape} \d+ (-?\d+)$', native, re.M)]
            assert printed == values, (shape, [i for i, (p, v) in enumerate(zip(printed, values)) if p != v])
        print(f'N={args.n}: native equals host SQLite and the plain reading for all {len(answers)} shapes')
    for shape in ORACLES:
        bits = answers[shape]
        k, psi = grover(bits)
        line = f'N={args.n} {shape}: {sum(bits)} marked, k={k}, success {sum(a * a for a, b in zip(psi, bits) if b):.9f}'
        if args.amplitudes:
            program = args.build/'scale-history'/f'{shape}.bn'
            dump = subprocess.run([args.harness, program, 'dump', 'scale_grover', str(2 * args.n)],
                                  capture_output=True, text=True, check=True).stdout
            got = {}
            for config, re_, im in re.findall(r'^amplitude \[([^\]]*)\] (\S+) (\S+)$', dump, re.M):
                columns = [int(c) for c in config.split(',')]
                assert len(columns) == 1 and columns[0] < args.n, columns
                got[columns[0]] = complex(float(re_), float(im))
            error = max(abs(got.get(r, 0) - psi[r]) for r in range(args.n))
            norm = sum(abs(a) ** 2 for a in got.values())
            assert error < TOLERANCE and abs(norm - 1) < TOLERANCE, (shape, error, norm)
            assert 'region_faults 0' in dump, dump[-200:]
            line += f', device amplitude error {error:.2e}'
        print(line)


if __name__ == '__main__':
    main()
