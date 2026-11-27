#!/usr/bin/env python3
"""Mutate changed Rust lines before landing; deliberately separate from cargo test."""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import sys


def run(*args, **kwargs):
    return subprocess.check_output(args, **kwargs)


def reject_constant(value):
    raise ValueError(f'{value} is not a JSON number')


def finite_float(value):
    number = float(value)
    if not math.isfinite(number):
        raise ValueError(f'number outside the finite float range: {value}')
    return number


def read_json(data, command):
    try:
        return json.loads(data.decode('utf-8'), parse_constant=reject_constant, parse_float=finite_float)
    except ValueError as error:
        sys.exit(f'{command} returned invalid JSON: {error}')


def exit_status(returncode):
    # subprocess uses negative return codes for signals. The shell uses 128 + signal.
    return returncode if returncode >= 0 else 128 - returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('base', help='accepted base revision (normally origin/main)')
    parser.add_argument('--list', action='store_true', help='preview mutations; does not count as a run')
    args = parser.parse_args()
    root = Path(os.fsdecode(run('git', 'rev-parse', '--show-toplevel').removesuffix(b'\n')))
    if run('git', 'status', '--porcelain').strip():
        sys.exit('commit the step first: mutation testing requires a clean exact revision')
    base = run('git', 'merge-base', args.base, 'HEAD').strip().decode('ascii')
    head = run('git', 'rev-parse', 'HEAD').strip().decode('ascii')
    output = root / '.tmp' / 'mutation' / f'{base[:12]}-{head[:12]}'
    output.mkdir(parents=True, exist_ok=True)
    diff = output / 'changed.diff'
    patch = run('git', 'diff', '--no-ext-diff', '--no-renames', base, head)
    diff.write_bytes(patch)
    if not patch:
        sys.exit('no changed lines: select the base of the step')
    metadata = read_json(run('cargo', 'metadata', '--no-deps', '--format-version', '1', cwd=root), 'cargo metadata')
    members = metadata['workspace_default_members']
    names = run('git', 'diff', '--no-ext-diff', '--no-renames', '--name-only', '-z', base, head)
    changed_rust = [root / os.fsdecode(name) for name in names.split(b'\0') if name.endswith(b'.rs')]
    selected = [p for p in metadata['packages'] if p['id'] in members or any(
        file.is_relative_to(Path(p['manifest_path']).parent) for file in changed_rust)]
    packages = [p['name'] for p in selected]
    # The tool defaults gitignore off; source-local scratch must not copy itself.
    command = ['cargo', 'mutants', '--workspace', '--in-diff', str(diff), '--output', str(output),
               '--gitignore', 'true', '--jobs', '1', '--jobserver-tasks', os.environ.get('CARGO_BUILD_JOBS', '2')]
    for name in packages:
        command.extend(['--test-package', name])
    print(f'mutation step {base}..{head}; results {output.relative_to(root)}', flush=True)
    env = os.environ | {'TMPDIR': str(output)}
    preview = subprocess.run([*command, '--list', '--json'], cwd=root, env=env, stdout=subprocess.PIPE)
    if preview.returncode:
        status = exit_status(preview.returncode)
        sys.stderr.buffer.write(preview.stdout)
        print(f'cargo mutants --list --json failed (exit {status})', file=sys.stderr)
        return status
    # cargo-mutants can leave stdout blank when it has no mutants to filter.
    candidates = read_json(preview.stdout if preview.stdout.strip() else b'[]', 'cargo mutants --list --json')
    if not isinstance(candidates, list):
        sys.exit('cargo mutants --list --json expected a JSON array')
    if not candidates:
        sys.exit('no generated Rust mutants in this diff: record explicit seeded edits for unsupported changes')
    print(f'{len(candidates)} changed-line mutants selected', flush=True)
    if args.list:
        for candidate in candidates:
            print(candidate)
        return 0
    front_tests = root / 'front/tests'
    if any(Path(p['manifest_path']).parent == front_tests for p in selected):
        front = Path(env.get('BENNETT_FRONT', root / 'target/front/bennett-front')).resolve()
        if not front.is_file():
            sys.exit('run front/test.sh at this revision before mutating the front harness')
        env['BENNETT_FRONT'] = str(front)
    code = subprocess.call(command, cwd=root, env=env)
    # A successful empty run is not evidence for changes outside cargo-mutants' Rust scope.
    print('Inspect missed/unviable/timeout outcomes. Non-Rust changes require explicit seeded edits; '
          'see docs/CHECKS.md. No mutation percentage is accepted as a substitute.', flush=True)
    return exit_status(code)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as error:
        status = exit_status(error.returncode)
        sys.stderr.buffer.write(error.stdout)
        print(f'{error.cmd[0]} {error.cmd[1]} failed (exit {status})', file=sys.stderr)
        sys.exit(status)
