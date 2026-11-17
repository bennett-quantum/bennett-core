#!/usr/bin/env python3
"""Lower the C-marker oracle against the same linked Lua interpreter and run it on the device."""
import argparse
import json
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('build', type=Path)
    args = parser.parse_args()
    build = args.build.resolve()
    subprocess.run(['clang-18', '-O0', '-Xclang', '-disable-O0-optnone', '-gline-tables-only',
                    f'-I{ROOT / "front/include"}', '-emit-llvm', '-c',
                    str(ROOT / 'experiments/lua-probe/marker.c'), '-o', str(build / 'marker.bc')], check=True)
    subprocess.run(['llvm-link-18', str(build / 'link/linked.bc'), str(build / 'marker.bc'),
                    '-o', str(build / 'marker-linked.bc')], check=True)
    start = time.monotonic()
    result = subprocess.run([str(ROOT / 'target/front/bennett-front'), str(build / 'marker-linked.bc'),
                             '-o', str(build / 'marker.bn'), '--entry', 'lua_coherent_c',
                             '--stack-bytes', '1048576', '--region-budget', '1000000'],
                            capture_output=True, text=True)
    report = dict(status=result.returncode, seconds=time.monotonic() - start,
                  stdout=result.stdout, stderr=result.stderr, results={})
    assert result.returncode == 0, report
    for seed in range(8):
        run = subprocess.run([str(ROOT / 'target/debug/bennett'), 'run', str(build / 'marker.bn'),
                              '--entry', 'lua_coherent_c', '--seed', str(seed)],
                             capture_output=True, text=True, check=True)
        result_line = next(line for line in run.stdout.splitlines() if line.startswith('result: '))
        report['results'][seed] = int(result_line.split()[1])
    (build / 'marker.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    # predicate.lua is 1 only for input 2 of the four the two Hadamard qubits give.
    assert set(report['results'].values()) <= {0, 1}, report


if __name__ == '__main__':
    main()
