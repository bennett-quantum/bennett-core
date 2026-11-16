#!/usr/bin/env python3
"""Raw-bit sweep against native libgcc (build the front and release executor first).

Run from anywhere: python3 rt/libgcc/exact/run.py [--random N] [--level O0|O2].
Scratch output goes under .tmp/libgcc-exact; compare.py reports NaN bits separately.
"""
import argparse
from pathlib import Path
import subprocess
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def run(args, **kw):
    return subprocess.run([str(a) for a in args], check=True, **kw)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--random', type=int, default=262144)
    parser.add_argument('--level', choices=['O0', 'O2'], default='O0')
    args = parser.parse_args()
    out = ROOT / '.tmp/libgcc-exact' / f'{args.level}-{args.random}'
    out.mkdir(parents=True, exist_ok=True)
    source = HERE / 'cplx_exact.c'
    flags = [f'-{args.level}', '-ffp-contract=off', f'-DRANDOM={args.random}']
    run(['clang-18', *flags, '-DNATIVE', source, '-lm', f'-Wl,-Map,{out / "native.map"}', '-o', out / 'native'])
    with (out / 'native.txt').open('w') as f:
        run([out / 'native'], stdout=f)
    run(['clang-18', *flags, '-Xclang', '-disable-O0-optnone', '-fno-vectorize', '-fno-slp-vectorize',
         '-emit-llvm', '-c', source, '-o', out / 'program.bc'])
    run([ROOT / 'target/front/bennett-front', out / 'program.bc', '--region-budget', '100000', '--stack-bytes', '1048576', '-o', out / 'program.bn'])
    start = time.monotonic()
    with (out / 'device.out').open('w') as f:
        run([ROOT / 'target/release/bennett', 'run', out / 'program.bn', '--budget', '100000000000'], stdout=f)
    raw = (out / 'device.out').read_text()
    assert raw.startswith('result: 0\n'), raw[:1000]
    console = next(line for line in raw.splitlines() if line.startswith('console: "'))
    (out / 'device.txt').write_text(console.removeprefix('console: "').removesuffix('"'))
    print(f'device wall seconds: {time.monotonic() - start:.3f}', flush=True)
    print('\n'.join(line for line in raw.splitlines() if not line.startswith('console:')), flush=True)
    print(f'outputs: {out.relative_to(ROOT)}', flush=True)
    run(['python3', '-I', HERE / 'compare.py', out / 'device.txt', out / 'native.txt', args.random])


if __name__ == '__main__':
    main()
