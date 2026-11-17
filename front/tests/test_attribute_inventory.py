#!/usr/bin/env python3
"""Exact independent oracle for the inventory reader's sites, placements and multiplicity.

Usage: python3 front/tests/test_attribute_inventory.py path/to/read-attributes
"""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
scratch = root / '.tmp'
scratch.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(dir=scratch) as directory:
    source = Path(directory) / 'placements.ll'
    source.write_text('''
declare noundef nonnull align 8 dereferenceable(8) ptr @source(ptr noundef returned align 8 dereferenceable(8), i64 noundef) #0
define noundef i32 @entry(ptr noundef nonnull %p, i64 noundef %n) #0 {
  %r = call noundef nonnull align 8 dereferenceable(8) ptr @source(ptr noundef nonnull align 8 dereferenceable(8) %p, i64 noundef %n) #1
  ret i32 17
}
attributes #0 = { nounwind "no-trapping-math"="true" }
attributes #1 = { nounwind }
''')
    expected = {}

    def expect(site, placement, attrs):
        for kind, attr in attrs:
            expected[(site, placement, kind, attr)] = 1

    enum = lambda *names: [('enum', name) for name in names]
    for site in ('declaration', 'definition'):
        expect(site, 'function', [*enum('nounwind'), ('string', '"no-trapping-math"="true"')])
    expect('declaration', 'return', enum('noundef', 'nonnull', 'align 8', 'dereferenceable(8)'))
    expect('declaration', 'parameter', enum('noundef', 'returned', 'align 8', 'dereferenceable(8)'))
    expect('definition', 'return', enum('noundef'))
    expect('definition', 'parameter', enum('noundef', 'nonnull'))
    expect('call', 'function', enum('nounwind'))
    for placement in ('return', 'parameter'):
        expect('call', placement, enum('noundef', 'nonnull', 'align 8', 'dereferenceable(8)'))
    for site in ('declaration', 'definition', 'call'):
        expected[(site, 'parameter', 'enum', 'noundef')] = 2
    # Two copies of a module prove counts accumulate rather than deduplicate sites.
    for copies in (1, 2):
        result = subprocess.run([sys.argv[1], *([str(source)] * copies)], capture_output=True, text=True, check=True)
        actual = {}
        for line in result.stdout.splitlines():
            *key, count = line.split('\t')
            actual[tuple(key)] = int(count)
        assert actual == {key: copies * value for key, value in expected.items()}, (actual, expected)
    log = Path(directory) / 'commands.jsonl'
    generated = Path(directory) / 'generated.c'
    versions = ['unsigned value(void) { return 7; }\n', 'unsigned value(void) { return 11; }\n']
    for text in versions:
        generated.write_text(text)
        subprocess.run([root / 'front/tests/attribute-record-clang.py', '-O0', '-emit-llvm', '-c',
                        generated, '-o', Path(directory) / 'generated.bc'], check=True,
                       env={**os.environ, 'ATTRIBUTE_CLANG_LOG': str(log), 'ATTRIBUTE_CLANG': 'clang-18'})
    commands = [json.loads(line) for line in log.read_text().splitlines()]
    assert [Path(c['source_snapshot']).read_text() for c in commands] == versions
    assert len({c['source_snapshot'] for c in commands}) == 2
print('attribute inventory: exact declaration/definition/call placements and multiplicity passed')
print('attribute recorder: overwritten generated source versions preserved')
