#!/usr/bin/env python3
"""Measure clang-18 attributes before front lowering; heavy, outside cargo test.

Build rt/musl first. Supply the corpus source directories and verified Lua archive.
--front-commands optionally replays bitcode commands recorded by a CLANG wrapper
(JSON lines with cwd/args), retaining harness-specific defines and target options.
Outputs include counts by placement/site/value and every unsuccessful compile.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import shlex
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[2]
LUA_SHA256 = '2335b6c582a52654f94612bf10d2f4672805d05329aa6568b1d8cd9e5c6fb8e6'
OLD_C = ['-w', '-Wno-error=implicit-function-declaration', '-Wno-error=implicit-int',
         '-Wno-error=int-conversion', '-Wno-error=incompatible-pointer-types',
         '-Wno-error=incompatible-function-pointer-types', '-Wno-error=return-type']
DIALECT = {'-fgnu89-inline', '-fwrapv', '-std=gnu89', '-std=c89', '-std=c99',
           '-fno-strict-overflow', '-fno-builtin-abort', '-fno-inline', '-finput-charset=utf-8'}


def checked(args, **kwargs):
    return subprocess.run([str(x) for x in args], check=True, capture_output=True, text=True, **kwargs)


def main():
    # Invalid-input fixtures may abort LLVM's child reader; keep those measurements
    # from leaving core files beside otherwise ordinary test artifacts.
    _, core_hard_limit = resource.getrlimit(resource.RLIMIT_CORE)
    resource.setrlimit(resource.RLIMIT_CORE, (0, core_hard_limit))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--gcc', required=True, type=Path, help='gcc.c-torture/execute')
    parser.add_argument('--cts', required=True, type=Path, help='c-testsuite/tests/single-exec')
    parser.add_argument('--lua-archive', required=True, type=Path)
    parser.add_argument('--front-commands', type=Path)
    parser.add_argument('--out', type=Path, default=ROOT / '.tmp/attribute-inventory')
    parser.add_argument('--jobs', type=int, default=3)
    parser.add_argument('--groups', help='optional comma-separated subset for a separate measurement')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    clang = os.environ.get('CLANG', 'clang-18')
    version = checked([clang, '--version']).stdout
    assert 'clang version 18.' in version, version
    llvm_flags = shlex.split(checked(['llvm-config-18', '--cxxflags', '--ldflags',
                                    '--libs', 'core', 'irreader', '--system-libs']).stdout)
    reader = out / 'read-attributes'
    checked(['clang++-18', ROOT / 'front/tests/attribute-inventory.cpp', *llvm_flags,
             '-std=c++17', '-o', reader])
    lib = ROOT / 'target/musl'
    sysroot = ['-nostdinc', '-isystem', str(lib / 'sysroot/include')]
    include = ['-I', str(ROOT / 'front/include')]
    units = []

    def add(group, source, flags):
        units.append((group, source.resolve(), [str(x) for x in flags]))

    for group, directory in [('gcc', args.gcc), ('ieee', args.gcc / 'ieee'), ('cts', args.cts)]:
        sources = sorted(directory.glob('*.c'))
        assert sources, directory
        for source in sources:
            flags = []
            for line in source.read_text(errors='replace').splitlines():
                if re.search(r'dg-(additional-)?options', line) and 'target' not in line:
                    flags += [word for word in line.translate(str.maketrans('', '', '"{}')).split()
                              if word in DIALECT]
            x = source.with_suffix('.x')
            if x.exists() and '-fno-trapping-math' in x.read_text():
                flags.append('-fno-trapping-math')
            if group == 'ieee':
                flags.append('-fno-inline')
            add(group, source, [*sysroot, *include, *OLD_C, *flags])
    musl_flags = json.loads((lib / 'cflags.json').read_text())
    for row in (lib / 'compile.tsv').read_text().splitlines():
        name, _ = row.split('\t')
        if name.endswith('.c'):
            add('musl', ROOT / 'rt/musl/musl-1.2.5' / name, musl_flags)
    for source in sorted((ROOT / 'rt/musl').glob('*.c')):
        add('musl-glue', source, [*musl_flags, '-DMAIN_ARGS=0'])
    # All checked-in C fixtures, even those whose tests are ignored. The recorded
    # commands below add actual default-harness configurations and generated C.
    for source in sorted(p for p in (ROOT / 'front/tests/c').rglob('*') if p.suffix in ('.c', '.cpp')):
        add('front-static', source, include)
    if args.front_commands:
        seen = set()
        for line in args.front_commands.read_text().splitlines():
            command = json.loads(line)
            argv = command['args']
            sources = [(Path(command['cwd']) / x).resolve() for x in argv if x.endswith(('.c', '.cpp'))]
            if len(sources) != 1 or not any(part in sources[0].parts for part in ('front-tests', 'tests')):
                continue
            flags, skip = [], False
            for index, word in enumerate(argv):
                if skip:
                    skip = False
                elif word == '-o' or (word == '-Xclang' and argv[index + 1] == '-disable-O0-optnone'):
                    skip = True
                elif word not in ('-emit-llvm', '-c') and not word.startswith('-O') and not word.endswith(('.c', '.cpp')):
                    flags.append(word)
            key = (str(sources[0]), tuple(flags))
            if 'source_snapshot' in command:
                snapshot = Path(command['source_snapshot'])
                # A recorder can accumulate multiple development runs. Historical
                # checked-in fixture revisions are not this checkout's inputs;
                # preserve all versions only for generated test sources.
                if sources[0].is_relative_to(ROOT / 'front/tests/c') and snapshot.read_bytes() != sources[0].read_bytes():
                    continue
                key = (str(snapshot), str(sources[0].parent), tuple(flags))
                flags += ['-iquote', str(sources[0].parent)]
                sources = [snapshot]
            if key not in seen:
                seen.add(key)
                add('front-harness', sources[0], flags)
    assert hashlib.sha256(args.lua_archive.read_bytes()).hexdigest() == LUA_SHA256
    with tarfile.open(args.lua_archive) as archive:
        archive.extractall(out, filter='data')
    lua = out / 'lua-5.4.9/src'
    names = [p for p in sorted(lua.glob('*.c')) if p.name not in ('lua.c', 'luac.c')]
    assert len(names) == 32
    lua_flags = [*sysroot, '-std=c99', '-DLUA_COMPAT_5_3', '-I', lua]
    for source in names:
        add('lua', source, lua_flags)
    # Recreate the two generated headers used by the probe's embeddings.
    probe = ROOT / 'experiments/lua-probe'
    (out / 'scripts.h').write_text('static const char *const probe_scripts[] = {\n' +
        ',\n'.join(json.dumps(p.read_text()) for p in sorted((probe / 'scripts').glob('*.lua'))) + '\n};\n')
    (out / 'predicate.h').write_text('static const char predicate_source[] = ' +
                                   json.dumps((probe / 'predicate.lua').read_text()) + ';\n')
    for name in ('driver.c', 'oracle.c'):
        add('lua', probe / name, [*lua_flags, *include, '-I', out])
    if args.groups:
        groups = set(args.groups.split(','))
        units = [unit for unit in units if unit[0] in groups]
        assert units, args.groups

    def measure(task):
        index, group, source, flags, level = task
        bc = out / f'{index}-{level}.bc'
        command = [clang, *flags, f'-{level}', '-fno-vectorize', '-fno-slp-vectorize',
                   '-Xclang', '-disable-O0-optnone', '-gline-tables-only', '-emit-llvm', '-c', source, '-o', bc]
        result = subprocess.run([str(x) for x in command], capture_output=True, text=True)
        row = dict(group=group, source=source.name, sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                   level=level, status=result.returncode)
        if result.returncode:
            # Logs are local artifacts; the published summary contains counts only.
            (out / f'{index}-{level}.log').write_text(result.stderr)
            return row, []
        result = subprocess.run([str(reader), str(bc)], capture_output=True, text=True)
        if result.returncode:
            row['bitcode_reader_status'] = result.returncode
            (out / f'{index}-{level}.log').write_text(result.stderr)
            # Clang's intentional over-alignment witness emits invalid bitcode;
            # the bitcode debug upgrader verifies and aborts before we can inspect
            # it. LLVM's text reader can still expose the emitted AttributeSets.
            # This is a measurement recovery, never admission to the front.
            ll = bc.with_suffix('.ll')
            text_command = [str(x) for x in command]
            text_command[-1] = str(ll)
            emitted = subprocess.run([*text_command, '-S', '-g0'], capture_output=True, text=True)
            if not emitted.returncode:
                result = subprocess.run([str(reader), str(ll)], capture_output=True, text=True)
            if emitted.returncode or result.returncode:
                row.update(status=emitted.returncode or result.returncode, stage='LLVM text recovery')
                return row, []
            row['inventory_format'] = 'text without debug after bitcode-reader rejection'
            ll.unlink()
        attributes = result.stdout.splitlines()
        bc.unlink()
        return row, attributes

    tasks = [(i, group, source, flags, level) for i, (group, source, flags) in enumerate(units)
             for level in ('O0', 'O2')]
    counts, records = Counter(), []
    with ThreadPoolExecutor(args.jobs) as pool:
        for row, attributes in pool.map(measure, tasks):
            records.append(row)
            for line in attributes:
                site, placement, kind, spelling, count = line.split('\t')
                counts[(row['group'], row['level'], site, placement, kind, spelling)] += int(count)
    (out / 'sources.json').write_text(json.dumps(records, indent=2) + '\n')
    with (out / 'counts.tsv').open('w') as target:
        target.write('group\tlevel\tsite\tplacement\tkind\tattribute\tcount\n')
        for key, count in sorted(counts.items()):
            target.write('\t'.join((*key, str(count))) + '\n')
    summary = Counter((r['group'], r['level'], 'failed' if r['status'] else 'compiled') for r in records)
    report = dict(compiler=version, units=len(units),
                  builds=[dict(group=g, level=o, status=s, count=n) for (g, o, s), n in sorted(summary.items())])
    (out / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2), flush=True)


if __name__ == '__main__':
    main()
