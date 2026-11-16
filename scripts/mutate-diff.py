#!/usr/bin/env python3
"""Mutate changed Rust lines before landing; deliberately separate from cargo test."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('base', help='accepted base revision (normally origin/main)')
    parser.add_argument('--list', action='store_true', help='preview mutations; does not count as a run')
    args = parser.parse_args()
    root = Path(run('git', 'rev-parse', '--show-toplevel').strip())
    if run('git', 'status', '--porcelain').strip():
        sys.exit('commit the step first: mutation testing requires a clean exact revision')
    base = run('git', 'merge-base', args.base, 'HEAD').strip()
    head = run('git', 'rev-parse', 'HEAD').strip()
    output = root / '.tmp' / 'mutation' / f'{base[:12]}-{head[:12]}'
    output.mkdir(parents=True, exist_ok=True)
    diff = output / 'changed.diff'
    diff.write_text(run('git', 'diff', '--no-ext-diff', '--no-renames', base, head))
    if not diff.stat().st_size:
        sys.exit('no changed lines: select the base of the step')
    metadata = json.loads(run('cargo', 'metadata', '--no-deps', '--format-version', '1', cwd=root))
    members = metadata['workspace_default_members']
    packages = [p['name'] for p in metadata['packages'] if p['id'] in members]
    # The tool defaults gitignore off; source-local scratch must not copy itself.
    command = ['cargo', 'mutants', '--in-diff', str(diff), '--output', str(output),
               '--gitignore', 'true', '--jobs', '1', '--jobserver-tasks', os.environ.get('CARGO_BUILD_JOBS', '2')]
    for name in packages:
        command.extend(['--test-package', name])
    print(f'mutation step {base}..{head}; results {output.relative_to(root)}', flush=True)
    env = os.environ | {'TMPDIR': str(output)}
    candidates = json.loads(run(*command, '--list', '--json', cwd=root, env=env))
    if not candidates:
        sys.exit('no generated Rust mutants in this diff: record explicit seeded edits for unsupported changes')
    print(f'{len(candidates)} changed-line mutants selected', flush=True)
    if args.list:
        for candidate in candidates:
            print(candidate)
        return 0
    code = subprocess.call(command, cwd=root, env=env)
    # A successful empty run is not evidence for changes outside cargo-mutants' Rust scope.
    print('Inspect missed/unviable/timeout outcomes. Non-Rust changes require explicit seeded edits; '
          'see docs/CHECKS.md. No mutation percentage is accepted as a substitute.', flush=True)
    return code


if __name__ == '__main__':
    sys.exit(main())
