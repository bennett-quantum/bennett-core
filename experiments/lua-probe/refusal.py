#!/usr/bin/env python3
"""Measure the C-marker boundary using the same linked Lua interpreter."""
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
                  stdout=result.stdout, stderr=result.stderr)
    (build / 'refusal.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    assert result.returncode == 3, report
    assert 'a call that depends on a qubit, to a function that writes memory or performs quantum operations' in result.stderr, report


if __name__ == '__main__':
    main()
