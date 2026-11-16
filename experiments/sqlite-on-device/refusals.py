#!/usr/bin/env python3
"""Measure both C-marker refusals without changing the original linked program."""
import argparse
import json
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('build', type=Path)
    args = ap.parse_args()
    build = args.build.resolve()
    results = {}
    for isolated in [False, True]:
        tag = 'marker-isolated' if isolated else 'marker'
        command = ['clang-18', '-O0', '-Xclang', '-disable-O0-optnone', '-gline-tables-only',
                   '-I'+str(ROOT/'front/include'), '-emit-llvm', '-c',
                   str(ROOT/'experiments/sqlite-on-device/marker.c'), '-o', str(build/(tag+'.bc'))]
        if isolated:
            command.append('-Dmain=experiment_coherent_c')
        subprocess.run(command, check=True)
        command = ['llvm-link-18', str(build/'link/linked.bc')]
        if not isolated:
            command.append('--override')
        command += [str(build/(tag+'.bc')), '-o', str(build/(tag+'-linked.bc'))]
        subprocess.run(command, check=True)
        command = [str(ROOT/'target/front/bennett-front'), str(build/(tag+'-linked.bc')),
                   '-o', str(build/(tag+'.bn')), '--region-budget', '100000', '--stack-bytes', '1048576']
        if isolated:
            command += ['--entry', 'experiment_coherent_c']
        start = time.monotonic()
        p = subprocess.run(command, capture_output=True, text=True)
        elapsed = time.monotonic()-start
        assert p.returncode == 3 and 'not handled yet:' in p.stderr, p.stdout+p.stderr
        expected = ('a call that depends on a qubit, to a function that writes memory or performs quantum operations'
                    if isolated else 'the address of @__stdio_seek, which performs quantum operations, is taken')
        assert expected in p.stderr, p.stderr
        results[tag] = dict(seconds=elapsed, status=p.returncode, reason=p.stderr)
    (build/'refusals.json').write_text(json.dumps(results, indent=2)+'\n')
    print(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
