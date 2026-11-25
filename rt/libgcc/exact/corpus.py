#!/usr/bin/env python3
"""Check the three GCC cdivchk programs on the device against native, with musl linked.

Give the GCC gcc.c-torture/execute/ieee directory as the only argument. Build musl,
the front and release executor first. Their own ilogb/ilogbl calls are musl's, under
FENV_ACCESS (constrained operations, lowered since the floating-point environment is carried).
"""
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
MUSL = ROOT / 'rt/musl'
OUT = ROOT / '.tmp/libgcc-corpus'


def run(args, **kw):
    return subprocess.run([str(a) for a in args], **kw)


def main():
    source_dir = Path(sys.argv[1]).resolve()
    OUT.mkdir(parents=True, exist_ok=True)
    failed = 0
    for name in ['cdivchkf', 'cdivchkd', 'cdivchkld']:
        source = source_dir / f'{name}.c'
        for level in ['O0', 'O2']:
            native = OUT / f'{name}-{level}'
            run(['clang-18', f'-{level}', '-fno-inline', source, '-lm', f'-Wl,-Map,{native}.map', '-o', native], check=True)
            reference = run([native], capture_output=True, check=True)
            assert reference.stdout == b'', reference.stdout
        linked = OUT / f'{name}.bn'
        result = run(['python3', MUSL / 'link.py', source, '-o', linked, '--clang-flag=-fno-inline'], capture_output=True, text=True)
        (OUT / f'{name}.log').write_text(result.stdout + result.stderr)
        if result.returncode:
            failed += 1
            print(f'{name}: REFUSED: {result.stderr.strip().splitlines()[-1]}', flush=True)
            continue
        actual = run([ROOT / 'target/release/bennett', 'run', linked, '--entry', '__bennett_entry',
                      '--budget', '100000000'], capture_output=True, text=True, check=True)
        (OUT / f'{name}.out').write_text(actual.stdout)
        if not (actual.stdout.startswith('result: 0\n') and 'console:' not in actual.stdout):
            failed += 1
            print(f'{name}: DIFFERS: {actual.stdout.strip()}', flush=True)
            continue
        print(f'{name}: agrees, {" ".join(actual.stdout.split())}', flush=True)
    return bool(failed)


if __name__ == '__main__':
    sys.exit(main())
