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


SUFFIX_LANGUAGES = {'.py': 'python', '.sh': 'shell', '.js': 'javascript', '.ts': 'javascript'}
# Interpreter names on a shebang line, matched whole after any directory and an `env` launcher.
SHEBANG_LANGUAGES = [(re.compile(r'python[0-9.]*'), 'python'), (re.compile(r'(?:ba|da|k|z|a)?sh'), 'shell'),
                     (re.compile(r'node|deno|bun|ts-node'), 'javascript')]


# Python calls whose duration may be passed by position, with that argument's index: `time.sleep(secs)`,
# `Popen.wait(timeout)`, `Popen.communicate(input, timeout)`, `Thread.join(timeout)`,
# `Lock.acquire(blocking, timeout)`, `socket.settimeout(value)`, `asyncio.wait_for(aw, timeout)`,
# `signal.alarm(seconds)`, `socket.create_connection(address, timeout)`, `urlopen(url, data, timeout)`.
# Names whose positional argument is often something else (`dict.get`, `select`) are keyword-only here.
POSITIONAL_TIMEOUTS = {'sleep': 0, 'wait': 0, 'communicate': 1, 'join': 0, 'acquire': 1, 'settimeout': 0,
                       'wait_for': 1, 'alarm': 0, 'create_connection': 1, 'urlopen': 2}


def script_language(path):
    """The scanner for a script, by extension or else by its shebang line; None for other files.

    An unknown shebang interpreter is returned as ('unknown', interpreter) so it is reported, not skipped.
    """
    if path.suffix in SUFFIX_LANGUAGES:
        return SUFFIX_LANGUAGES[path.suffix]
    with path.open('rb') as handle:
        first = handle.readline()
    # `#![` opens a Rust inner attribute, not an interpreter line.
    if not first.startswith(b'#!') or first.startswith(b'#!['):
        return None
    words = first[2:].decode(errors='replace').split()
    if words and Path(words[0]).name == 'env':
        words = [word for word in words[1:] if not word.startswith('-') and '=' not in word]
    interpreter = Path(words[0]).name if words else ''
    for pattern, language in SHEBANG_LANGUAGES:
        if pattern.fullmatch(interpreter):
            return language
    return ('unknown', interpreter)


def bare_times(root, names):
    issues = []
    for name in names:
        path = root / name
        if path.is_symlink() or not path.is_file():
            continue
        language = script_language(path)
        if language is None:
            continue
        if isinstance(language, tuple):
            issues.append(f'{name}:1: script for interpreter {language[1]!r} has no bare-time scanner; add one')
            continue
        text = path.read_text()
        if language == 'python':
            tree = ast.parse(text, filename=str(name))
            for node in ast.walk(tree):
                if not isinstance(node, ast.Call):
                    continue
                func = node.func
                called = func.attr if isinstance(func, ast.Attribute) else getattr(func, 'id', None)
                position = POSITIONAL_TIMEOUTS.get(called)
                args = ([node.args[position]] if position is not None and len(node.args) > position else []) + [
                    kw.value for kw in node.keywords if kw.arg == 'timeout']
                if any(literal(arg) for arg in args):
                    issues.append(f'{name}:{node.lineno}: bare time constant; name it with its unit and purpose')
        else:
            # Keep physical newlines for diagnostics, but accept shell line continuations.
            source = '\n'.join('' if line.lstrip().startswith(('#', '//')) else line
                               for line in text.splitlines())
            space = r'(?:[ \t]|\\\r?\n)+'
            pattern = (r'\b(?:timeout|sleep)' + space + r'(?:--?[^ \t\n]+' + space + r')*'
                       r'[\'\"]?[+-]?(?:\d|\.\d)|'
                       r'\bset(?:Timeout|Interval)\s*\([^,]*,\s*(?:\d|\.\d)')
            for match in re.finditer(pattern, source):
                n = source.count('\n', 0, match.start()) + 1
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
