#!/usr/bin/env python3
"""Separate native codegen's NaN choices from device arithmetic.

Build the front, then run exact/run.py first (a strict NaN mismatch is expected).
This uses its saved device output, never replaces the GCC archive as the oracle,
and changes only scratch IR in an explicitly labelled native diagnostic.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
# The front's build directory: where $BENNETT_FRONT lives when it is set (as the front tests honour it),
# else target/front as front/test.sh builds it.
FRONT_BUILD = Path(os.environ['BENNETT_FRONT']).resolve().parent if 'BENNETT_FRONT' in os.environ \
    else ROOT / 'target/front'
RUNTIME = HERE.parent
NAMES = ['mulsc3', 'divsc3', 'muldc3', 'divdc3', 'mulxc3', 'divxc3']

# Native diagnostic only: force each commutative IR operation's operand 0 into the
# SSE destination. Its NaN priority is then the same as SoftFloat's 8086-SSE path.
ORDERED = '''
float ordered_fadd_f32(float a,float b){__asm__("addss %1, %0":"+x"(a):"x"(b));return a;}
float ordered_fmul_f32(float a,float b){__asm__("mulss %1, %0":"+x"(a):"x"(b));return a;}
double ordered_fadd_f64(double a,double b){__asm__("addsd %1, %0":"+x"(a):"x"(b));return a;}
double ordered_fmul_f64(double a,double b){__asm__("mulsd %1, %0":"+x"(a):"x"(b));return a;}
'''


def run(args, **kw):
    return subprocess.run([str(a) for a in args], check=True, **kw)


def compare(left, right, random, must_match=False):
    result = subprocess.run(['python3', '-I', str(HERE / 'compare.py'), '--raw', str(left), str(right), str(random)],
                            capture_output=True, text=True)
    print(result.stdout, end='', flush=True)
    assert not result.stderr, result.stderr
    assert all(f'{name}: ' in result.stdout for name in NAMES), result.stdout
    assert result.stdout.count(', 0 differ otherwise') == len(NAMES), result.stdout
    # compare.py --raw uses exit 1 for *any* mismatch. Other errors must never count as evidence.
    assert result.returncode in (0, 1), result.returncode
    if must_match:
        assert result.returncode == 0, 'operand-order hardware diagnostic differs from device'


def native(out, source, objects, args, label):
    exe = out / f'{label}-{source.stem}'
    run(['clang-18', f'-{args.level}', '-ffp-contract=off', '-DNATIVE', f'-DRANDOM={args.random}',
         source, *objects, '-lm', f'-Wl,-Map,{exe}.map', '-o', exe])
    text = exe.with_suffix('.txt')
    with text.open('w') as f:
        run([exe], stdout=f)
    return text


def ordered_objects(out):
    ordered = out / 'ordered.c'
    ordered.write_text(ORDERED)
    wrapper = ordered.with_suffix('.o')
    run(['clang-18', '-O0', '-c', ordered, '-o', wrapper])
    objects = [wrapper]
    for name in NAMES:
        ir = out / f'ordered-{name}.ll'
        run(['llvm-dis-18', FRONT_BUILD / f'complex/{name}.bc', '-o', ir])
        text = ir.read_text()

        def replace(m):
            prefix, op, ty, a, b = m.groups()
            bits = 32 if ty == 'float' else 64
            return f'{prefix}call {ty} @ordered_{op}_f{bits}({ty} {a}, {ty} {b})'

        text, count = re.subn(r'^(\s*%\w+ = )(fadd|fmul) (float|double) ([^,\n]+), ([^\n]+)$',
                             replace, text, flags=re.M)
        assert count > 0 if name not in ('mulxc3', 'divxc3') else count == 0, (name, count)
        for op in ['fadd', 'fmul']:
            for ty, bits in [('float', 32), ('double', 64)]:
                text += f'\ndeclare {ty} @ordered_{op}_f{bits}({ty}, {ty})\n'
        ir.write_text(text)
        obj = ir.with_suffix('.o')
        # No further IR optimization. Native subtraction/division/conversion remain intact.
        run(['clang-18', '-O0', '-c', ir, '-o', obj])
        objects.append(obj)
    return objects


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--random', type=int, default=262144)
    parser.add_argument('--level', choices=['O0', 'O2'], default='O0')
    args = parser.parse_args()
    saved = ROOT / '.tmp/libgcc-exact' / f'{args.level}-{args.random}'
    assert (saved / 'device.txt').exists(), 'run exact/run.py first'
    out = ROOT / '.tmp/libgcc-diagnose' / f'{args.level}-{args.random}'
    out.mkdir(parents=True, exist_ok=True)
    builds = [('GCC archive', [])]
    for level in ['O0', 'O2']:
        objects = []
        for name in NAMES:
            obj = out / f'clang-{level}-{name}.o'
            run(['clang-18', f'-{level}', '-ffp-contract=off', '-ffreestanding', '-fno-builtin',
                 '-fno-vectorize', '-fno-slp-vectorize', f'-DL_{name}',
                 '-I', RUNTIME / 'config', '-I', RUNTIME / 'gcc-13.3.0/gcc',
                 '-I', RUNTIME / 'gcc-13.3.0/include', '-c',
                 RUNTIME / 'gcc-13.3.0/libgcc/libgcc2.c', '-o', obj])
            objects.append(obj)
        builds.append((f'Clang {level}', objects))
    builds.append(('IR operand order', ordered_objects(out)))
    for index, (label, objects) in enumerate(builds):
        print(f'\n{label} hardware vs saved GCC archive', flush=True)
        full = native(out, HERE / 'cplx_exact.c', objects, args, str(index))
        compare(full, saved / 'native.txt', args.random, must_match=index == 0)
        print(f'{label} hardware vs device', flush=True)
        compare(full, saved / 'device.txt', args.random, must_match=index == 3)
        witness = native(out, HERE / 'nan_witness.c', objects, args, str(index))
        print(f'{label} witnesses (mulsc3, muldc3, divsc3, divdc3 #9, divdc3 #19604):\n'
              + witness.read_text(), flush=True)
    bc, bn = out / 'witness.bc', out / 'witness.bn'
    run(['clang-18', f'-{args.level}', '-Xclang', '-disable-O0-optnone', '-ffp-contract=off',
         '-emit-llvm', '-c', HERE / 'nan_witness.c', '-o', bc])
    run([FRONT_BUILD / 'bennett-front', bc, '--region-budget', '100000', '--stack-bytes', '1048576', '-o', bn])
    result = run([ROOT / 'target/release/bennett', 'run', bn, '--budget', '10000000'],
                 capture_output=True, text=True).stdout
    (out / 'witness-device.out').write_text(result)
    print('Device witnesses:\n' + result, flush=True)
    assert result.startswith('result: 0\n'), result
    # The console uses escaped newlines; compare exactly, without stripping NaNs.
    console = next(line for line in result.splitlines() if line.startswith('console: "'))
    actual = console.removeprefix('console: "').removesuffix('"').replace('\\n', '\n')
    assert actual == witness.read_text(), (actual, witness.read_text())
    print('All operand-order hardware/device bits agree; GCC archive comparison remains strict.', flush=True)


if __name__ == '__main__':
    main()
