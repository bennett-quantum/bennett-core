#!/usr/bin/env python3
"""Optional CLANG wrapper for attribute-inventory.py; preserve generated source versions.

Set ATTRIBUTE_CLANG_LOG to a local JSONL path and ATTRIBUTE_CLANG to clang-18.
The compiler's normal output and exit status pass through unchanged.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import sys

argv = sys.argv[1:]
if '-emit-llvm' in argv and '-c' in argv:
    sources = [Path(x).resolve() for x in argv if x.endswith(('.c', '.cpp'))]
    if len(sources) == 1:
        log = Path(os.environ['ATTRIBUTE_CLANG_LOG']).resolve()
        snapshots = log.with_suffix('.sources')
        snapshots.mkdir(parents=True, exist_ok=True)
        source = sources[0]
        content = source.read_bytes()
        snapshot = snapshots / (hashlib.sha256(content).hexdigest() + source.suffix)
        snapshot.write_bytes(content)
        with log.open('a') as target:
            fcntl.flock(target, fcntl.LOCK_EX)
            target.write(json.dumps(dict(cwd=os.getcwd(), args=argv, source_snapshot=str(snapshot))) + '\n')
os.execvp(os.environ.get('ATTRIBUTE_CLANG', 'clang-18'), ['clang-18', *argv])
