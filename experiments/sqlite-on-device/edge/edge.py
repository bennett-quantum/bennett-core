#!/usr/bin/env python3
"""Edge rows through the SQL oracle: build, independent truth, device checks.

The truth table of every query is computed twice without the native build: by
the host Python's own SQLite and by a plain-Python reading of the query. Both
must equal the native driver's output before ../scale.py's checks (complete
native/device output, exact classical reversal, two coherent calls against the
dense matrices, Grover with every amplitude) run on every query.

    SQLITE_DIR=<amalgamation> python3 experiments/sqlite-on-device/edge/edge.py target/sqlite-on-device/edge
"""
import argparse
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))
import scale  # noqa: E402

INT64_MAX = (1 << 63) - 1
INT64_MIN = -(1 << 63)
NO_ROW = -1000
SQLITE_ERROR = 1
# (a, b, name, note); None is SQL NULL. Must equal sod_rows in edge_driver.c.
ROWS = [
    (3, INT64_MAX, "m", "ZQ upper"),
    (None, INT64_MIN, "", None),
    (3, 0, "M", "a%zq_b"),
    (-1, 500, None, "zzq"),
    (-1, 499, "l\u00ff", "z q"),
    (3, -500, "ma", ""),
    (8, 1000, "lzzzz", "zQ"),
    (INT64_MAX, None, "a", "q z"),
]
NAMES = ["eq_null", "any_null", "text_order", "like_case", "duplicate_a", "all_rows",
         "no_rows", "two_where", "add_overflow", "range_agg", "real_compare", "abs_error"]
RANGE = NAMES.index("range_agg")


def sql_text():
    source = (HERE/'edge_driver.c').read_text()
    body = re.search(r'sod_sql\[\] = \{(.*?)\};', source, re.S)[1]
    return [s.encode().decode('unicode_escape') for s in re.findall(r'"((?:[^"\\]|\\.)*)"', body)]


def host_sqlite():
    db = sqlite3.connect(':memory:')
    db.execute('CREATE TABLE e(id INTEGER PRIMARY KEY,a INTEGER,b INTEGER,name TEXT,note TEXT)')
    db.executemany('INSERT INTO e VALUES(?,?,?,?,?)', [(i + 1, *r) for i, r in enumerate(ROWS)])
    table = {}
    for q, sql in enumerate(sql_text()):
        sql = sql.replace('?1', '?')
        values = []
        for r in range(len(ROWS)):
            arg = r if q == RANGE else r + 1
            try:
                row = db.execute(sql, (arg,) * sql.count('?')).fetchone()
                values.append(NO_ROW if row is None else int(row[0] or 0))
            except sqlite3.OperationalError as error:
                assert 'integer overflow' in str(error), error
                values.append(-SQLITE_ERROR)
        table[NAMES[q]] = values
    return table


def by_reading():
    """The same predicates read from SQLite's documented semantics, in Python."""
    def cmp_null(f, *xs):
        return 0 if any(x is None for x in xs) else int(f(*xs))
    t = {name: [] for name in NAMES}
    for r, (a, b, name, note) in enumerate(ROWS):
        t['eq_null'].append(cmp_null(lambda a: a == 3, a))
        t['any_null'].append(int(None in (a, b, name, note)))
        t['text_order'].append(cmp_null(lambda n: n.encode() < b'm', name))  # BINARY: memcmp of UTF-8
        t['like_case'].append(cmp_null(lambda n: 'zq' in n.lower(), note))  # ASCII case folding
        t['duplicate_a'].append(int(a is not None and sum(x[0] == a for x in ROWS) > 1))
        t['all_rows'].append(1)
        t['no_rows'].append(0)
        t['two_where'].append(int(a is not None and b is not None and a < 8 and b >= 500))
        # Integer overflow in + gives a REAL; INT64_MAX+1.0 = 2^63 > INT64_MAX, compared exactly.
        t['add_overflow'].append(cmp_null(lambda a: True, a))
        t['range_agg'].append(int(any(x[0] is not None and r <= x[0] <= r + 1 for x in ROWS)))
        # CAST to REAL rounds INT64_MAX up to 2^63, which SQLite compares exactly as unequal.
        t['real_compare'].append(cmp_null(lambda b: float(b) == b and b != INT64_MAX, b))
        t['abs_error'].append(-SQLITE_ERROR if b == INT64_MIN else cmp_null(lambda b: True, b))
    return t


def build(out):
    env = dict(os.environ)
    sqlite_dir = Path(env['SQLITE_DIR'])
    out.mkdir(parents=True, exist_ok=True)
    (out/'program.c').write_text('#include "sqlite3.c"\n#include "edge_driver.c"\n')
    flags = ['-DSQLITE_THREADSAFE=0', '-DSQLITE_OMIT_LOAD_EXTENSION', '-DSQLITE_OS_OTHER=1',
             '-DSQLITE_ENABLE_MEMSYS5', '-DSQLITE_TEMP_STORE=3', f'-I{sqlite_dir}', f'-I{HERE}', f'-I{HERE.parent}']
    subprocess.run(['clang-18', '-O2', '-fno-vectorize', '-fno-slp-vectorize', *flags, out/'program.c',
                    '-o', out/'native'], check=True)
    subprocess.run([sys.executable, ROOT/'rt/musl/link.py', out/'program.c', '-o', out/'program.bn',
                    '--arena-bytes', '4096', '--stack-bytes', str(1 << 20), '--keep', out/'link',
                    *[f'--clang-flag={f}' for f in flags]], check=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('build', type=Path)
    ap.add_argument('--no-build', action='store_true')
    ap.add_argument('--modes', nargs='+', default=['forward', 'reverse', 'coherent', 'grover'])
    args, rest = ap.parse_known_args()
    out = args.build.resolve()
    host, read = host_sqlite(), by_reading()
    assert host == read, {k: (host[k], read[k]) for k in NAMES if host[k] != read[k]}
    if not args.no_build:
        build(out)
    native = subprocess.run([out/'native'], capture_output=True, text=True)
    assert native.returncode == 0, native
    got = {name: [int(v) for r, v in re.findall(rf'^{name} (\d+) (-?\d+)$', native.stdout, re.M)] for name in NAMES}
    assert got == host, {k: (got[k], host[k]) for k in NAMES if got[k] != host[k]}
    print('native equals host SQLite and the reading for all', len(NAMES), 'queries')
    # Every query is an oracle, its rows marked where the result is 1: abs_error's error row (-1, the
    # negated SQLITE_ERROR) is unmarked, where truncation to one bit would have marked it.
    assert [v for v in host['abs_error'] if v not in (0, 1)] == [-SQLITE_ERROR]
    scale.SHAPES.clear()
    scale.SHAPES.update({name: q for q, name in enumerate(NAMES)})
    sys.argv = [sys.argv[0], str(out), '--n', str(len(ROWS)), '--modes', *args.modes, *rest]
    scale.main()


if __name__ == '__main__':
    main()
