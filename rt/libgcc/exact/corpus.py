#!/usr/bin/env python3
"""Check the three GCC cdivchk programs; report the normal route and an explicit diagnostic.

Give the GCC gcc.c-torture/execute/ieee directory as the only argument. Build musl,
the front and release executor first. This does NOT lift the FENV_ACCESS debt.
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
    for name in ['cdivchkf', 'cdivchkd', 'cdivchkld']:
        source = source_dir / f'{name}.c'
        for level in ['O0', 'O2']:
            native = OUT / f'{name}-{level}'
            run(['clang-18', f'-{level}', source, '-lm', f'-Wl,-Map,{native}.map', '-o', native], check=True)
            reference = run([native], capture_output=True, check=True)
            assert reference.stdout == b'', reference.stdout
        linked = OUT / f'{name}.bn'
        result = run(['python3', MUSL / 'link.py', source, '-o', linked], capture_output=True, text=True)
        (OUT / f'{name}-normal.log').write_text(result.stdout + result.stderr)
        if result.returncode:
            assert 'constrained floating-point intrinsic' in result.stderr, result.stderr
            print(f'{name} normal route: REFUSED at musl FENV_ACCESS', flush=True)
        else:
            actual = run([ROOT / 'target/release/bennett', 'run', linked, '--entry', '__bennett_entry',
                          '--budget', '100000000'], capture_output=True, text=True, check=True)
            (OUT / f'{name}-normal.out').write_text(actual.stdout)
            assert actual.stdout.startswith('result: 0\n') and 'console:' not in actual.stdout, actual.stdout
            print(f'{name} normal route: agrees, {actual.stdout.strip()}', flush=True)
        # Diagnostic ONLY: retain the original corpus C; supply musl's exact ilogb bodies
        # with their FENV_ACCESS pragmas removed in scratch copies. The fallback and
        # observable exception state are NOT implemented by this diagnostic.
        shim = OUT / f'{name}-diagnostic.c'
        bodies = []
        for f in ['ilogb.c', 'ilogbl.c']:
            body = (MUSL / 'musl-1.2.5/src/math' / f).read_text()
            bodies.append(body.replace('#pragma STDC FENV_ACCESS ON', '/* diagnostic: pragma removed */'))
        shim.write_text('\n'.join(bodies) + '\n' + source.read_text())
        program = OUT / f'{name}-diagnostic.bn'
        run(['python3', MUSL / 'link.py', shim, '-o', program, '--clang-flag=-Dhidden=', '--clang-flag=-I' + str(MUSL / 'musl-1.2.5/src/internal'),
             '--clang-flag=-I' + str(MUSL / 'musl-1.2.5/arch/x86_64'),
             '--clang-flag=-I' + str(MUSL / 'musl-1.2.5/arch/generic')], check=True)
        actual = run([ROOT / 'target/release/bennett', 'run', program, '--entry', '__bennett_entry',
                      '--budget', '100000000'], capture_output=True, text=True, check=True)
        (OUT / f'{name}-diagnostic.out').write_text(actual.stdout)
        assert actual.stdout.startswith('result: 0\n') and 'console:' not in actual.stdout, actual.stdout
        print(f'{name} diagnostic (pragma removed): agrees, {actual.stdout.strip()}', flush=True)


if __name__ == '__main__':
    main()
