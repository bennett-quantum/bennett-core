#!/usr/bin/env python3
"""Check that the native reference IS the vendored source: the six complex helpers the default clang link
takes from the host's libgcc.a are rebuilt from rt/libgcc (unmodified GCC 13.3.0 source plus Bennett's
config/ headers, no -fbuilding-libgcc) with the host's GCC and compared with the archive members: the
disassembly with relocations and every read-only data section must be identical.

Needs the host GCC that built the archive (Ubuntu's 13.3.0 here; VENDOR.md). Ubuntu builds with frame
pointers, hence -fno-omit-frame-pointer. python3 rt/libgcc/exact/archive.py [--out DIR]; exit 1 on a difference.
"""
import argparse
from pathlib import Path
import re
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
RUNTIME = HERE.parent
NAMES = ['mulsc3', 'divsc3', 'muldc3', 'divdc3', 'mulxc3', 'divxc3']


def run(args, **kw):
    return subprocess.run([str(a) for a in args], check=True, capture_output=True, text=True, **kw).stdout


def contents(obj):
    """Disassembly with relocations from the first function on, then each .rodata section's bytes."""
    text = run(['objdump', '-dr', obj])
    parts = [text[text.index('\n0000'):]]
    for section in sorted(set(re.findall(r'\s(\.rodata[\w.]*)\s', run(['readelf', '-SW', obj])))):
        parts.append(section + run(['objdump', '-s', '-j', section, obj]).split(section, 1)[1])
    return '\n'.join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, default=ROOT / '.tmp/libgcc-archive')
    args = parser.parse_args()
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    version = run(['gcc', '-dumpfullversion']).strip()
    assert version == '13.3.0', f'host gcc {version}: the reference archive is GCC 13.3.0 (VENDOR.md)'
    archive = Path(run(['clang-18', '-print-libgcc-file-name']).strip()).resolve()
    (out / 'program.c').write_text(
        '#include <complex.h>\nvolatile float f; volatile double d; volatile long double x;\n'
        'int main(void) { float _Complex a = f + f * I; double _Complex b = d + d * I;\n'
        '  long double _Complex c = x + x * I;\n'
        '  return (int)(crealf(a * a / a) + creal(b * b / b) + creall(c * c / c)); }\n')
    run(['clang-18', '-O2', out / 'program.c', '-lm', f'-Wl,-Map,{out / "native.map"}', '-o', out / 'program'])
    linked = (out / 'native.map').read_text()
    failed = False
    for name in NAMES:
        member = f'_{name}.o'
        sources = {Path(p).resolve() for p in re.findall(rf'(\S+)\({re.escape(member)}\)', linked)}
        assert sources == {archive}, f'{name}: the native link takes {member} from {sources}, not {archive}'
        run(['ar', 'x', '--output', out, archive, member])
        rebuilt = out / f'rebuilt{member}'
        run(['gcc', '-O2', '-g0', '-fno-omit-frame-pointer', '-ffreestanding', '-fno-builtin', '-Werror',
             f'-DL_{name}', '-I', RUNTIME / 'config', '-I', RUNTIME / 'gcc-13.3.0/gcc',
             '-I', RUNTIME / 'gcc-13.3.0/include', '-c', RUNTIME / 'gcc-13.3.0/libgcc/libgcc2.c', '-o', rebuilt])
        same = contents(out / member) == contents(rebuilt)
        failed |= not same
        print(f'{archive}({member}): {"identical to" if same else "DIFFERS from"} the vendored source rebuilt by gcc {version}')
    raise SystemExit(int(failed))


if __name__ == '__main__':
    main()
