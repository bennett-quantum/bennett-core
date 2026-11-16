#!/usr/bin/env python3
"""Fast tree checks. No baselines, hit-count thresholds, or per-file exemptions."""
import ast
import os
from pathlib import Path
import re
import subprocess
import sys


def files(root):
    manifest = os.environ.get('BENNETT_CHECK_MANIFEST')
    if manifest:
        return [Path(name) for name in Path(manifest).read_text().splitlines()]
    names = subprocess.check_output(
        ['git', '-C', str(root), 'ls-files', '-z', '--cached', '--others', '--exclude-standard']
    ).decode().split('\0')
    return sorted({Path(name) for name in names if name})


def hygiene(root, names):
    issues = []
    litter = re.compile(
        r'(^|/)(?:__pycache__|\.tmp|\.beads|node_modules|target)(?:/|$)'
        r'|\.(?:pyc|pyo|log|bak|orig|rej|swp|o|bc|a|so|tar|gz|zip)$'
        r'|(?:^|/)(?:PROGRESS|TRANSCRIPT|LAST-RUN)(?:\.[^/]*)?$'
        r'|\.lock$|~$', re.I)
    host = re.compile(r'/(?:home|Users)/[A-Za-z0-9_.-]+|[A-Za-z]:\\Users\\[^\\\s]+')
    for name in names:
        path = root / name
        if litter.search(name.as_posix()) and name.name != 'Cargo.lock':
            issues.append(f'{name}: stray generated/session file')
        if path.is_symlink():
            target = os.readlink(path)
            if os.path.isabs(target) or '..' in Path(target).parts:
                issues.append(f'{name}: nonlocal symlink')
            continue
        if not path.is_file():
            continue  # a tracked deletion in a working-tree check
        try:
            text = path.read_text()
        except UnicodeError:
            continue
        for n, line in enumerate(text.splitlines(), 1):
            if host.search(line):
                issues.append(f'{name}:{n}: host-specific path')
    return issues


def literal(node):
    return isinstance(node, ast.Constant) and isinstance(node.value, (int, float)) or (
        isinstance(node, (ast.UnaryOp, ast.BinOp)) and any(literal(c) for c in ast.iter_child_nodes(node)))


def bare_times(root, names):
    issues = []
    for name in names:
        path = root / name
        if path.suffix not in {'.py', '.sh', '.js', '.ts'} or not path.is_file():
            continue
        text = path.read_text()
        if path.suffix == '.py':
            tree = ast.parse(text, filename=str(name))
            for node in ast.walk(tree):
                if not isinstance(node, ast.Call):
                    continue
                func = node.func
                is_sleep = (isinstance(func, ast.Attribute) and func.attr == 'sleep') or (
                    isinstance(func, ast.Name) and func.id == 'sleep')
                args = ([node.args[0]] if is_sleep and node.args else []) + [
                    kw.value for kw in node.keywords if kw.arg == 'timeout']
                if any(literal(arg) for arg in args):
                    issues.append(f'{name}:{node.lineno}: bare time constant; name it with its unit and purpose')
        else:
            for n, line in enumerate(text.splitlines(), 1):
                if line.lstrip().startswith(('#', '//')):
                    continue
                if re.search(r'\b(?:timeout|sleep)\s+(?:-[\w-]+\s+)*[\'\"]?\d|'
                             r'\bset(?:Timeout|Interval)\s*\([^,]*,\s*\d', line):
                    issues.append(f'{name}:{n}: bare time constant; name it with its unit and purpose')
    return issues


def main():
    root = Path(__file__).resolve().parent.parent
    names = files(root)
    if not names:
        raise SystemExit('no source files discovered')
    issues = hygiene(root, names) + bare_times(root, names)
    for issue in issues:
        print(issue, file=sys.stderr)
    print(f'tree checks: {len(names)} files, {len(issues)} problems')
    return bool(issues)


if __name__ == '__main__':
    sys.exit(main())
