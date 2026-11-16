#!/usr/bin/env python3
"""Compare every embedded script with native; --reverse is the separate stage 2."""
import argparse
import ast
import json
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('build', type=Path)
    parser.add_argument('--reverse', action='store_true')
    args = parser.parse_args()
    build = args.build.resolve()
    cases = json.loads((build / 'cases.json').read_text())
    base = (build / 'program.bn').read_text()
    text = base
    for index in range(len(cases)):
        text += f'''
func @lua_case{index}() -> i32 {{
entry:
 call @lua_probe_select(i32 {index})
 %status = call @__bennett_entry()
 ret %status
}}
'''
    program = build / 'scripts.bn'
    program.write_text(text)
    rows = []
    destination = build / ('reverse.json' if args.reverse else 'forward.json')
    for index, name in enumerate(cases):
        native = subprocess.run([str(build / 'native'), str(index)], capture_output=True)
        assert native.returncode >= 0 and not native.stderr, native
        command = [ROOT / 'target/debug/bennett', 'run', program, '--entry', f'lua_case{index}',
                   '--budget', '100000000', '--stats']
        if args.reverse:
            command.append('--reverse')
        start = time.monotonic()
        result = subprocess.run([str(x) for x in command], capture_output=True, text=True)
        status = re.search(r'^result: (\d+)', result.stdout, re.M)
        console = re.search(r'^console: (".*")$', result.stdout, re.M)
        steps = re.search(r'^steps: (\d+)', result.stdout, re.M)
        journal = re.search(r'^journal: (\d+) bytes', result.stdout, re.M)
        output = ast.literal_eval('b' + console[1]) if console else b''
        row = dict(script=name, native_status=native.returncode, native_stdout_hex=native.stdout.hex(),
                   status=int(status[1]) if status else None, stdout_hex=output.hex(),
                   process_status=result.returncode, steps=int(steps[1]) if steps else None,
                   journal_bytes=int(journal[1]) if journal else None,
                   seconds=time.monotonic() - start, report=result.stdout, stderr=result.stderr)
        row['agrees'] = (result.returncode == 0 and row['status'] is not None and
                         row['status'] % 256 == native.returncode and output == native.stdout)
        if args.reverse:
            row['restored'] = 'classical state identical, amplitudes within 0e0' in result.stdout
        rows.append(row)
        destination.write_text(json.dumps(rows, indent=2) + '\n')
        print(json.dumps(row), flush=True)
        if not row['agrees'] or (args.reverse and not row['restored']):
            raise SystemExit(f'stage {2 if args.reverse else 1} failed at {name}; see {destination.name}')


if __name__ == '__main__':
    main()
