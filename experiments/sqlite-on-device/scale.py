#!/usr/bin/env python3
"""Measure larger, unmodified SQL predicates through the handwritten device entry.

Build with SQL_ORACLE=1 first. Each child has explicit time/address-space bounds;
failures retain their reports and are reported, never treated as passing checks.
"""
import argparse
import ast
import json
import math
import os
from pathlib import Path
import re
import resource
import signal
import subprocess
import time

from grover import diffusion

ROOT = Path(__file__).resolve().parents[2]
# Per-child bounds are experiment limits on a shared host, not performance gates.
DEFAULT_RUN_SECONDS = 300
TERMINATION_GRACE_SECONDS = 5
PARENT_TIMEOUT_GRACE_SECONDS = 15
SHAPES = {'like_note': 2, 'int_eq': 4, 'text_order': 5, 'two_where': 6, 'range_agg': 7}


def program(base, q, n, k):
    if n < 4 or not n.bit_count() == 1:
        raise ValueError('N must be a power of two >= 4')
    width = n.bit_length() - 1
    text = base + f'''\nfunc @scale_lookup(i{width} %row) -> i1 {{
entry:
 %r = zext i{width} %row to i32
 %v = call @experiment_query(i32 {q}, %r)
 %b = trunc i32 %v to i1
 ret %b
}}
func @scale_classical() -> i32 {{
entry:
 %s = call @experiment_setup()
'''
    for r in range(n):
        text += (f' %v{r} = call @experiment_query(i32 {q}, i32 {r})\n'
                 f' %b{r} = add i32 %v{r}, 48\n'
                 f' %c{r} = trunc i32 %b{r} to i8\n effect %c{r}\n')
    text += ' ret 0\n}\n'
    for prep in ['zero', 'one', 'minus']:
        name = 'scale_grover' if prep == 'minus' else f'scale_{prep}'
        lines = [f'func @{name}() -> i32 {{', 'entry:',
                 ' %s = call @experiment_setup()', f' %row = qalloc {width}',
                 ' %out = qalloc 1', ' %fault = qalloc 5']
        lines += [f' h %row[{b}]' for b in range(width)]
        if prep != 'zero':
            lines += [' x %out']
        if prep == 'minus':
            lines += [' h %out']
        for _ in range(k if prep == 'minus' else 2):
            lines += [' apply @scale_lookup(%row) -> %out, %fault budget 100000000']
            if prep == 'minus':
                lines += diffusion(width)
        if prep == 'minus':
            lines += [' h %out', ' x %out', ' qfree %fault', ' qfree %out',
                      ' %m = measure %row', f' %result = zext i{width} %m to i32', ' ret %result']
        else:
            # Two calls must cancel the output XOR even with a dirty output bit.
            lines += [' ret 0']
        text += '\n'.join(lines + ['}']) + '\n'
    return text


def optimal_iterations(bits):
    marked, n = sum(bits), len(bits)
    if marked == 0 or marked == n:
        return 1
    theta = math.asin(math.sqrt(marked / n))
    # Nearest odd angle to pi/2, at least one complete oracle/diffusion.
    return max(1, round(math.pi / (4 * theta) - 0.5))


def measured(cmd, stem, seconds, memory_mib):
    start = time.monotonic()
    def limits():
        resource.setrlimit(resource.RLIMIT_AS, (memory_mib << 20, memory_mib << 20))
    with stem.with_suffix('.stdout').open('w') as out, stem.with_suffix('.stderr').open('w') as err:
        child = subprocess.Popen(['/usr/bin/time', '-f', '%e %M', '-o', str(stem.with_suffix('.time')),
                                  'timeout', '--signal=TERM', f'--kill-after={TERMINATION_GRACE_SECONDS}', str(seconds),
                                  *map(str, cmd)], stdout=out, stderr=err,
                                 start_new_session=True, preexec_fn=limits)
        timed_out = False
        try:
            code = child.wait(timeout=seconds + PARENT_TIMEOUT_GRACE_SECONDS)
            timed_out = code == 124
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(child.pid, signal.SIGKILL)
            code = child.wait()
    wall = time.monotonic() - start
    stdout, stderr = stem.with_suffix('.stdout').read_text(), stem.with_suffix('.stderr').read_text()
    timer = stem.with_suffix('.time').read_text()
    match = re.search(r'^(\S+) (\d+)$', timer, re.M)
    return dict(passed=code == 0 and not timed_out, status=code, timed_out=timed_out,
                seconds=wall, peak_rss_kib=int(match[2]) if match else None,
                limit_seconds=seconds, limit_mib=memory_mib, stdout=stdout, stderr=stderr)


def agrees_with_native(device, native):
    console = re.search(r'^console: (".*")$', device['stdout'], re.M)
    returned = re.search(r'^result: (\d+)$', device['stdout'], re.M)
    return bool(device['passed'] and console and returned
                and ast.literal_eval('b'+console[1]).decode() == native['stdout']
                and int(returned[1]) == native['status'])


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('build', type=Path)
    ap.add_argument('--n', type=int, required=True)
    ap.add_argument('--shapes', nargs='+', choices=list(SHAPES), default=list(SHAPES))
    ap.add_argument('--modes', nargs='+', choices=['forward', 'reverse', 'coherent', 'grover'],
                    default=['forward', 'reverse', 'coherent', 'grover'])
    ap.add_argument('--seconds', type=int, default=DEFAULT_RUN_SECONDS)
    ap.add_argument('--memory-mib', type=int, default=4096)
    ap.add_argument('--harness', type=Path, default=ROOT/'target/release/sqlite-on-device-harness')
    ap.add_argument('--apply', choices=['history', 'compare', 'branches'], default='history')
    args = ap.parse_args()
    os.environ['BENNETT_APPLY'] = args.apply
    build = args.build.resolve()
    output = build / f'scale-{args.apply}'
    output.mkdir(exist_ok=True)
    native = measured([build/'native'], output/'native', args.seconds, args.memory_mib)
    results = dict(n=args.n, apply=args.apply, native=native, cases=[])
    report = output/'results.json'
    def save():
        report.write_text(json.dumps(results, indent=2) + '\n')
    save()
    if not native['passed']:
        # Preserve a paired device observation even when setup cannot produce a
        # complete native truth table (e.g. SQLITE_NOMEM in the fixed heap).
        results['device_failure'] = measured(
            [ROOT/'target/debug/bennett', 'run', build/'program.bn', '--entry',
             '__bennett_entry', '--budget', '1000000000', '--stats'],
            output/'device-setup-failure', args.seconds, args.memory_mib)
        results['device_failure']['matches_native'] = agrees_with_native(results['device_failure'], native)
        save()
        if native['status'] == 2 and 'sqlite error: 7' in native['stdout']:
            assert results['device_failure']['matches_native'], 'SQLite allocation refusal disagrees'
        print('native driver failed: ' + native['stdout'] + native['stderr'], flush=True)
        print(results['device_failure']['stdout'], flush=True)
        raise SystemExit(1)
    answers = {}
    for shape in args.shapes:
        pairs = [(int(r), int(v)) for r, v in re.findall(rf'^{shape} (\d+) (-?\d+)$', native['stdout'], re.M)]
        assert [r for r, _ in pairs] == list(range(args.n)), (shape, pairs)
        bits = [v for _, v in pairs]
        assert all(v in [0, 1] for v in bits), (shape, bits)
        answers[shape] = bits
    base = (build/'program.bn').read_text()
    # Check the complete ordinary driver too, including memory/VFS reports and status.
    failed = False
    if 'forward' in args.modes:
        full = measured([ROOT/'target/debug/bennett', 'run', build/'program.bn', '--entry',
                         '__bennett_entry', '--budget', '1000000000', '--stats'],
                        output/'full', args.seconds, args.memory_mib)
        if full['passed']:
            full['passed'] = agrees_with_native(full, native)
        results['full_forward'] = full
        save()
        print(json.dumps(dict(n=args.n, mode='full', **{k: v for k, v in full.items() if k not in ['stdout', 'stderr']})), flush=True)
        if not full['passed']:
            failed = True
    for shape, bits in answers.items():
        k = optimal_iterations(bits)
        path = output/f'{shape}.bn'
        path.write_text(program(base, SHAPES[shape], args.n, k))
        for mode in args.modes:
            preps = ['zero', 'one'] if mode == 'coherent' else ['minus' if mode == 'grover' else 'classical']
            for prep in preps:
                entry = f'scale_{"grover" if mode == "grover" else prep}'
                item = measured([args.harness.resolve(), path, 'scale', mode,
                                 entry, ''.join(map(str, bits)), prep], output/f'{shape}-{mode}-{prep}',
                                args.seconds, args.memory_mib)
                item.update(shape=shape, mode=mode, prep=prep, marked=sum(bits),
                            iterations=k if mode == 'grover' else None)
                results['cases'].append(item)
                save()
                print(json.dumps(dict(n=args.n, **{k: v for k, v in item.items() if k not in ['stdout', 'stderr']})), flush=True)
                if not item['passed']:
                    failed = True
                    print(item['stdout'][-2000:] + item['stderr'][-2000:], flush=True)
    raise SystemExit(int(failed))


if __name__ == '__main__':
    main()
