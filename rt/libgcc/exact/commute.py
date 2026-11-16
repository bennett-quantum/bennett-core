#!/usr/bin/env python3
"""Check that the float/double NaN differences between the device and the GCC archive are operand orders.

Build the front first (front/test.sh or CMake). Native only: no device run. diagnose.py shows that the
production IR executed natively in IR operand order equals the device bit for bit. This script shows the
other half: the SAME production IR, with one fixed choice of operand order per fadd/fmul instruction (as a
compiled program has), reproduces the installed GCC archive's raw bits, NaNs included, on every tuple of
cplx_exact.c's sample. Long-double helpers need no check: x87 NaN selection does not depend on operand order.

python3 rt/libgcc/exact/commute.py [--random N] [--out DIR]; exits non-zero if any helper is not reproduced.
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

# Performs the operation with operand 0 as the SSE destination (SoftFloat's 8086-SSE NaN priority) unless
# the instruction's bit in cm_mask swaps them; records instructions whose two operands are both NaN.
WRAP = r'''
typedef unsigned long long u64;
u64 cm_mask, cm_both;
static int nf(float x) { unsigned u; __builtin_memcpy(&u, &x, 4); return (u & 0x7fffffffu) > 0x7f800000u; }
static int nd(double x) { u64 u; __builtin_memcpy(&u, &x, 8); return (u & 0x7fffffffffffffffull) > 0x7ff0000000000000ull; }
#define W(name, T, isnan, insn) T name(int i, T a, T b) { \
  if (isnan(a) && isnan(b)) cm_both |= 1ull << i; \
  if ((cm_mask >> i) & 1) { T t = a; a = b; b = t; } \
  __asm__(insn " %1, %0" : "+x"(a) : "x"(b)); return a; }
W(cm_fadd_f32, float, nf, "addss") W(cm_fmul_f32, float, nf, "mulss")
W(cm_fadd_f64, double, nd, "addsd") W(cm_fmul_f64, double, nd, "mulsd")
'''


def run(args, **kw):
    return subprocess.run([str(a) for a in args], check=True, **kw)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--random', type=int, default=262144)
    parser.add_argument('--out', type=Path, default=ROOT / '.tmp/libgcc-commute')
    args = parser.parse_args()
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    (out / 'wrap.c').write_text(WRAP)
    run(['clang-18', '-O1', '-c', out / 'wrap.c', '-o', out / 'wrap.o'])
    objects = [out / 'wrap.o']
    for name in ['mulsc3', 'divsc3', 'muldc3', 'divdc3']:
        text = run(['llvm-dis-18', FRONT_BUILD / f'complex/{name}.bc', '-o', '-'],
                   capture_output=True, text=True).stdout
        index = 0

        def replace(m):
            nonlocal index
            prefix, op, ty, a, b = m.groups()
            index += 1
            return f'{prefix}call {ty} @cm_{op}_f{32 if ty == "float" else 64}(i32 {index - 1}, {ty} {a}, {ty} {b})'

        text = re.sub(r'^(\s*%[\w.]+ = )(fadd|fmul) (float|double) ([^,\n]+), ([^\n]+)$', replace, text, flags=re.M)
        # Every commutative float/double operation must be routed, or the check would be vacuous.
        assert 0 < index <= 64 and not re.search(r'= (fadd|fmul) ', text), (name, index)
        assert f'@__{name}(' in text, name
        text = text.replace(f'@__{name}(', f'@cm___{name}(')
        for op in ['fadd', 'fmul']:
            for ty, bits in [('float', 32), ('double', 64)]:
                text += f'\ndeclare {ty} @cm_{op}_f{bits}(i32, {ty}, {ty})\n'
        (out / f'{name}.ll').write_text(text)
        run(['clang-18', '-O0', '-c', out / f'{name}.ll', '-o', out / f'{name}.o'])
        objects.append(out / f'{name}.o')
    # The archive's helpers come from the default link, exactly as for cplx_exact.c's native reference.
    run(['clang-18', '-O0', '-ffp-contract=off', '-DNATIVE', '-DNO_XC3', f'-DRANDOM={args.random}', '-I', HERE,
         HERE / 'commute.c', *objects, '-lm', f'-Wl,-Map,{out / "native.map"}', '-o', out / 'commute'])
    result = subprocess.run([str(out / 'commute')], capture_output=True, text=True)
    print(result.stdout, end='', flush=True)
    raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
