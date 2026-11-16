#!/usr/bin/env python3
"""Check each pushed commit in an isolated archive, including non-HEAD refs and dirty trees."""
import io
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def wip_commits(root, sha, remote_sha):
    revision = sha if set(remote_sha) == {'0'} else f'{remote_sha}..{sha}'
    rows = git(root, 'log', '--format=%h %s', revision).splitlines()
    return [row for row in rows if re.match(
        r'\S+\s+(?:\W*wip\b|work[- ]in[- ]progress\b|fixup!|squash!)', row, re.I)]


def check_revision(root, sha):
    # Every artifact is derived from this SHA. Neither the index nor the checkout supplies source.
    scratch = root / '.tmp' / 'push-check'
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f'{sha[:12]}-', dir=scratch) as tmp:
        tree = Path(tmp)
        archive = subprocess.check_output(['git', '-C', str(root), 'archive', '--format=tar', sha])
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(tree, filter='data')
        manifest = tree / '.check-manifest'
        manifest.write_text(git(root, 'ls-tree', '-r', '--name-only', sha) + '\n')
        env = os.environ | {'BENNETT_CHECK_MANIFEST': str(manifest)}
        # Do not let a caller's build location reuse artifacts from an unrelated revision.
        env.pop('CARGO_TARGET_DIR', None)
        env.pop('BENNETT_FRONT', None)
        print(f'pre-push: checking exact revision {sha}', flush=True)
        subprocess.run(['sh', str(tree / 'scripts/check.sh')], cwd=tree, env=env, check=True)


def main(root=None, updates=None):
    root = Path(root or git(Path.cwd(), 'rev-parse', '--show-toplevel'))
    seen = set()
    for line in sys.stdin if updates is None else updates:
        local_ref, sha, remote_ref, remote_sha = line.split()
        if set(sha) == {'0'}:  # deletion
            continue
        git(root, 'cat-file', '-e', f'{sha}^{{commit}}')
        if remote_ref == 'refs/heads/main':
            unfinished = wip_commits(root, sha, remote_sha)
            if unfinished:
                raise RuntimeError('unfinished commits cannot land on main:\n' + '\n'.join(unfinished))
        if sha not in seen:
            check_revision(root, sha)
            seen.add(sha)
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (RuntimeError, subprocess.CalledProcessError, ValueError) as error:
        sys.exit(f'pre-push refused: {error}')
