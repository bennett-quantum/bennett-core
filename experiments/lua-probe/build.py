#!/usr/bin/env python3
"""Build unmodified Lua 5.4.9, then use the existing musl archive resolver/front.

The supplied archive must match the published release checksum.
No network access is needed here. Run this heavy experiment outside cargo test.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import time

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
VERSION = '5.4.9'
SHA256 = '2335b6c582a52654f94612bf10d2f4672805d05329aa6568b1d8cd9e5c6fb8e6'


def run(argv, **kwargs):
    return subprocess.run([str(x) for x in argv], check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    parser.add_argument('--out', type=Path, default=ROOT / 'target/lua-probe')
    parser.add_argument('--oracle', action='store_true', help='build the separate predicate embedding')
    parser.add_argument('--predicate', type=Path, default=HERE / 'predicate.lua',
                        help='the Lua source of the oracle predicate (with --oracle)')
    parser.add_argument('--scripts', type=Path, action='append',
                        help='embed every *.lua here instead of scripts/ (repeatable, in the order given)')
    args = parser.parse_args()
    archive, out = args.archive.resolve(), args.out.resolve()
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise SystemExit('upstream archive checksum mismatch: expected Lua ' + VERSION)
    out.mkdir(parents=True, exist_ok=True)
    # Extract verified bytes afresh so an edited external tree cannot enter the run.
    with tarfile.open(archive) as source:
        source.extractall(out, filter='data')
    src = out / f'lua-{VERSION}' / 'src'
    names = sorted(p for p in src.glob('*.c') if p.name not in ('lua.c', 'luac.c'))
    assert len(names) == 32, names
    clang = os.environ.get('CLANG', 'clang-18')
    version = run([clang, '--version'], capture_output=True, text=True).stdout
    assert 'clang version 18.' in version, version
    common = ['-std=c99', '-DLUA_COMPAT_5_3', '-fno-vectorize', '-fno-slp-vectorize', f'-I{src}']
    if args.scripts:
        cases = [p for directory in args.scripts for p in sorted(directory.glob('*.lua'))]
        assert cases and len({p.stem for p in cases}) == len(cases), cases
    else:
        cases = sorted((HERE / 'scripts').glob('*.lua'))
        assert len(cases) == 7
    (out / 'scripts.h').write_text('static const char *const probe_scripts[] = {\n' +
                                ',\n'.join(json.dumps(p.read_text()) for p in cases) + '\n};\n')
    (out / 'cases.json').write_text(json.dumps([p.stem for p in cases]) + '\n')
    (out / 'predicate.h').write_text('static const char predicate_source[] = ' +
                                    json.dumps(args.predicate.read_text()) + ';\n')
    driver = HERE / ('oracle.c' if args.oracle else 'driver.c')
    native_objects, members = [], []
    musl = ROOT / 'target/musl'
    start = time.monotonic()
    for source in names:
        obj, bc = out / (source.stem + '.o'), out / (source.stem + '.bc')
        run([clang, *common, '-O2', '-c', source, '-o', obj])
        run([clang, *common, '-nostdinc', '-isystem', musl / 'sysroot/include',
             '-O0', '-Xclang', '-disable-O0-optnone', '-gline-tables-only',
             '-emit-llvm', '-c', source, '-o', bc])
        native_objects.append(obj)
        members.append(bc)
    if args.oracle:
        run([clang, *common, f'-I{out}', '-O2', driver, *native_objects, '-lm', '-o', out / 'native'])
    else:
        run([clang, *common, f'-I{out}', '-O2', '-Dmain=lua_device_main',
             '-c', driver, '-o', out / 'driver.o'])
        run([clang, '-O2', HERE / 'native.c', out / 'driver.o', *native_objects, '-lm', '-o', out / 'native'])
    # The resolver already understands arbitrary bitcode members: give it Lua's
    # members alongside musl in a probe-local archive view, without changing it.
    lib = out / 'lib'
    lib.mkdir(exist_ok=True)
    sysroot = lib / 'sysroot'
    if not sysroot.exists():
        sysroot.symlink_to(musl / 'sysroot', target_is_directory=True)
    (lib / 'cflags.json').write_bytes((musl / 'cflags.json').read_bytes())
    (lib / 'members.txt').write_text((musl / 'members.txt').read_text() +
                                   ''.join(str(p) + '\n' for p in members))
    (lib / 'index.tsv').unlink(missing_ok=True)
    command = ['python3', ROOT / 'rt/musl/link.py', driver,
               '-o', out / 'program.bn', '--libdir', lib, '--keep', out / 'link']
    command += [f'--clang-flag={f}' for f in [*common, f'-I{out}']]
    begin = time.monotonic()
    result = subprocess.run([str(x) for x in command], capture_output=True, text=True)
    report = dict(lua=VERSION, archive_sha256=SHA256, compiler=version,
                  translation_units=len(names), link_status=result.returncode,
                  link_seconds=time.monotonic() - begin, total_seconds=time.monotonic() - start,
                  stdout=result.stdout, stderr=result.stderr)
    (out / 'build.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
