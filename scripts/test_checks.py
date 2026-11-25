#!/usr/bin/env python3
"""Executed defect witnesses for the fast checks and exact-revision push hook."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import checks

ROOT = Path(__file__).resolve().parent.parent


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


push = module('push', ROOT / 'scripts/pre-push.py')


class Checks(unittest.TestCase):
    def setUp(self):
        (ROOT / '.tmp').mkdir(exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / '.tmp')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def test_hygiene_rejects_host_paths_and_litter(self):
        for name in ['session.log', 'PROGRESS.md', 'cache.pyc', '.tmp/stray.rs']:
            self.assertTrue(checks.hygiene(self.root, [Path(name)]), name)
        path = self.root / 'source.rs'
        path.write_text('const PATH: &str = "' + '/ho' + 'me/example/private";')
        self.assertIn('host-specific path', checks.hygiene(self.root, [Path('source.rs')])[0])
        path.write_text('ordinary source')
        self.assertEqual(checks.hygiene(self.root, [Path('source.rs')]), [])
        # Cargo.lock is source; unrelated lock files are litter.
        self.assertEqual(checks.hygiene(self.root, [Path('Cargo.lock')]), [])
        self.assertTrue(checks.hygiene(self.root, [Path('session.lock')]))

    def test_bare_times_reject_literals_and_accept_named_units(self):
        for name, bad, good in [
            ('run.sh', 'timeout 60 ./program\nsleep .5\ntimeout \\\n  2 ./program\n', 'timeout "$RUN_SECONDS" ./program\n'),
            ('run.py', 'import time\ntime.sleep(2)\nf(timeout=3 * 10)\n',
             'import time\ntime.sleep(WAIT_SECONDS)\nf(timeout=RUN_SECONDS)\n'),
        ]:
            (self.root / name).write_text(bad)
            self.assertEqual(len(checks.bare_times(self.root, [Path(name)])), 3 if name.endswith(".sh") else 2)
            (self.root / name).write_text(good)
            self.assertEqual(checks.bare_times(self.root, [Path(name)]), [])

    def test_bare_times_reads_extensionless_shell_scripts(self):
        # .githooks/pre-push is such a script in this tree; a literal there must be flagged like one in run.sh.
        (self.root / '.githooks').mkdir()
        hook = self.root / '.githooks/pre-push'
        hook.write_text('#!/bin/sh\nsleep 5\n')
        self.assertEqual(len(checks.bare_times(self.root, [Path('.githooks/pre-push')])), 1)

    def test_bare_times_selects_scripts_by_shebang(self):
        for name, text, found in [
            ('tool', '#!/usr/bin/env python3\nimport time\ntime.sleep(2)\n', 'bare time constant'),
            ('tool.in', '#!/usr/bin/env -S VAR=1 bash -e\ntimeout 9 ./x\n', 'bare time constant'),
            ('tool', '#! /usr/bin/perl\nsleep 5;\n', "interpreter 'perl' has no bare-time scanner"),
        ]:
            (self.root / name).write_text(text)
            self.assertIn(found, ' '.join(checks.bare_times(self.root, [Path(name)])), text)
        # A Rust inner attribute, ordinary text and binary data are not scripts.
        for text in [b'#![deny(warnings)]\nfn sleep(_: u8) {}\n', b'sleep 5\n', b'\x7fELF\xff\x00']:
            (self.root / 'lib').write_bytes(text)
            self.assertEqual(checks.bare_times(self.root, [Path('lib')]), [], text)

    def test_bare_times_flags_positional_timeouts(self):
        # Popen.wait(timeout) and Popen.communicate(input, timeout) take the timeout positionally.
        (self.root / 'run.py').write_text('import subprocess\np = subprocess.Popen(["x"])\np.wait(30)\np.communicate(None, 30)\n')
        self.assertEqual(len(checks.bare_times(self.root, [Path('run.py')])), 2)
        (self.root / 'run.py').write_text('worker.join(5)\nlock.acquire(True, -1 + 3)\nsignal.alarm(2)\n')
        self.assertEqual(len(checks.bare_times(self.root, [Path('run.py')])), 3)
        # Other positions, named durations and non-duration uses of the same names pass.
        (self.root / 'run.py').write_text(
            'p.communicate(b"30")\np.wait(timeout=RUN_SECONDS)\n", ".join(parts)\nlock.acquire(1)\nd.get(k, 0)\n')
        self.assertEqual(checks.bare_times(self.root, [Path('run.py')]), [])

    def test_hygiene_reads_files_that_are_not_utf8(self):
        # One Latin-1 byte must not hide a home path in the rest of a text file.
        (self.root / 'notes.txt').write_bytes(b'caf\xe9 /ho' b'me/example/private\n')
        self.assertIn('host-specific path', ' '.join(checks.hygiene(self.root, [Path('notes.txt')])))

    def test_hygiene_reads_wide_text_and_binary_data(self):
        home = '/ho' 'me/example/private'
        for data in [('first\n' + home).encode('utf-16'), ('x\n' + home).encode('utf-32'),
                     b'\x7fELF\x00\xff\xfe\n\x85' + home.encode() + b'\x00']:
            (self.root / 'blob').write_bytes(data)
            self.assertEqual(checks.hygiene(self.root, [Path('blob')]), ['blob:2: host-specific path'], data)
        (self.root / 'blob').write_bytes(b'\xff\xfe\x00\x00\x00\xd8')
        self.assertEqual(checks.hygiene(self.root, [Path('blob')]), [])

    def test_amend_subject_is_rejected_on_main(self):
        base = self.init_repo()
        (self.root / 'note').write_text('amended')
        self.commit('amend! Accepted base')
        tip = self.git('rev-parse', 'HEAD')
        self.assertEqual(len(push.wip_commits(self.root, tip, base)), 1)
        with self.assertRaisesRegex(RuntimeError, 'unfinished commits'):
            push.main(self.root, [f'HEAD {tip} refs/heads/main {base}'])
        # git itself writes the subject: commit --fixup=amend: and --fixup=reword:.
        self.git('-c', 'core.editor=true', 'commit', '-q', '--allow-empty', '--fixup=reword:' + base)
        self.assertEqual(len(push.wip_commits(self.root, self.git('rev-parse', 'HEAD'), base)), 2)

    def git(self, *args):
        return push.git(self.root, *args)

    def init_repo(self):
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'core.hooksPath', '/dev/null')
        self.git('config', 'commit.gpgsign', 'false')
        self.git('config', 'user.name', 'Check fixture')
        self.git('config', 'user.email', 'fixture@example.invalid')
        (self.root / '.gitignore').write_text('.tmp/\n')
        (self.root / 'scripts').mkdir()
        # An intentionally tiny test command proves which source the real archiver supplies.
        (self.root / 'scripts/check.sh').write_text('set -eu\ngrep -qx good subject\n')
        (self.root / 'subject').write_text('good\n')
        return self.commit('Accepted base')

    def commit(self, subject):
        self.git('add', '.')
        self.git('commit', '-qm', subject)
        return self.git('rev-parse', 'HEAD')

    def test_exact_revision_rejects_bad_non_head_despite_clean_head(self):
        good = self.init_repo()
        (self.root / 'subject').write_text('bad\n')
        bad = self.commit('Introduce seeded defect')
        self.git('checkout', '-q', '--detach', good)
        # A dirty file, cached artifacts, and HEAD cannot repair the pushed revision.
        (self.root / 'subject').write_text('good\n')
        with self.assertRaises(subprocess.CalledProcessError):
            push.main(self.root, [f'refs/heads/other {bad} refs/heads/topic {good}'])
        push.main(self.root, [f'HEAD {good} refs/heads/topic {bad}'])

    def test_exact_revision_passes_good_sha_despite_dirty_bad_checkout(self):
        good = self.init_repo()
        (self.root / 'subject').write_text('bad\n')
        push.main(self.root, [f'HEAD {good} refs/heads/topic {"0" * 40}'])
        # Branch deletion does not run the checks.
        push.main(self.root, [f'(delete) {"0" * 40} refs/heads/topic {good}'])

    def test_wip_ancestor_is_rejected_on_main_but_allowed_on_topic(self):
        base = self.init_repo()
        (self.root / 'note').write_text('unfinished')
        wip = self.commit('WIP: seeded unfinished ancestor')
        (self.root / 'note').write_text('finished tip')
        tip = self.commit('Finish tip')
        self.assertEqual(len(push.wip_commits(self.root, tip, base)), 1)
        self.assertIn(wip[:7], push.wip_commits(self.root, tip, '0' * 40)[0])
        with self.assertRaisesRegex(RuntimeError, 'unfinished commits'):
            push.main(self.root, [f'HEAD {tip} refs/heads/main {base}'])
        push.main(self.root, [f'HEAD {tip} refs/heads/wip/checks {base}'])
        self.assertEqual(push.wip_commits(self.root, base, '0' * 40), [])

    def mutation_step(self, file, before, after):
        self.init_repo()
        path = self.root / file
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(before)
        base = self.commit('Add mutation fixture')
        path.write_text(after)
        self.commit('Change mutation fixture')
        return base

    def run_mutation(self, base, *flags, listing='', diagnostic='', status=0, package_dir='.'):
        # Run the CLI on a real Git diff with a fake Cargo so these tests need no mutation tool or builds.
        bin_dir = self.root / '.tmp/bin'
        bin_dir.mkdir(parents=True, exist_ok=True)
        cargo = bin_dir / 'cargo'
        metadata = {'workspace_default_members': ['fixture'], 'packages': [
            {'id': 'fixture', 'name': 'fixture',
             'manifest_path': str(self.root / package_dir / 'Cargo.toml')}]}
        cargo.write_text(
            '#!/usr/bin/env python3\n'
            'import sys\n'
            'if sys.argv[1] == "metadata":\n'
            f'    print({json.dumps(metadata)!r})\n'
            'elif sys.argv[1] == "mutants" and sys.argv[-2:] == ["--list", "--json"]:\n'
            f'    sys.stdout.write({listing!r})\n'
            f'    sys.stderr.write({diagnostic!r})\n'
            f'    sys.exit({status})\n'
            'else:\n'
            '    sys.exit("unexpected cargo invocation: " + repr(sys.argv[1:]))\n')
        cargo.chmod(0o755)
        env = os.environ | {'PATH': os.pathsep.join([str(bin_dir), os.environ['PATH']]),
                            'BENNETT_FRONT': str(self.root / 'missing-front')}
        return subprocess.run(
            [sys.executable, str(ROOT / 'scripts/mutate-diff.py'), base, *flags],
            cwd=self.root, env=env, text=True, capture_output=True)

    def assert_empty_mutation_set(self, result, diagnostic=''):
        self.assertEqual(result.returncode, 1, result)
        self.assertEqual(result.stderr, diagnostic +
                         'no generated Rust mutants in this diff: record explicit seeded edits for unsupported changes\n')
        self.assertNotIn('changed-line mutants selected', result.stdout)

    def test_mutation_docs_only_rejects_blank_listing(self):
        base = self.mutation_step('README.md', 'Before\n', 'After\n')
        diagnostic = ' INFO Diff changes no Rust source files\n'
        for flags in [('--list',), ()]:
            with self.subTest(flags=flags):
                self.assert_empty_mutation_set(
                    self.run_mutation(base, *flags, diagnostic=diagnostic), diagnostic)

    def test_mutation_rust_comment_only_rejects_empty_array(self):
        source = 'pub fn increment(x: u32) -> u32 { x + 1 }\n'
        base = self.mutation_step('src/lib.rs', source, '// Increment an integer.\n' + source)
        for flags in [('--list',), ()]:
            with self.subTest(flags=flags):
                self.assert_empty_mutation_set(self.run_mutation(base, *flags, listing='[]\n'))

    def test_mutation_empty_front_harness_diff_does_not_require_front_build(self):
        base = self.mutation_step('front/tests/tests/smoke.rs', '#[test]\nfn smoke() {}\n',
                                  '#[test]\nfn smoke() { assert_eq!(1 + 1, 2); }\n')
        self.assert_empty_mutation_set(self.run_mutation(base, listing='[]\n', package_dir='front/tests'))

    def test_mutation_lists_candidates_for_changed_rust_code(self):
        base = self.mutation_step('src/lib.rs', 'pub fn f(x: u32) -> u32 { x + 1 }\n',
                                  'pub fn f(x: u32) -> u32 { x + 2 }\n')
        candidate = {'file': 'src/lib.rs', 'replacement': '-'}
        result = self.run_mutation(base, '--list', listing=json.dumps([candidate]))
        self.assertEqual(result.returncode, 0, result)
        self.assertEqual(result.stderr, '')
        self.assertIn('1 changed-line mutants selected\n', result.stdout)
        self.assertIn(str(candidate), result.stdout)

    def test_mutation_preserves_tool_failure(self):
        base = self.mutation_step('src/lib.rs', 'pub fn f() {}\n', 'pub fn g() {}\n')
        diagnostic = 'Error: failed to parse input diff\n'
        for flags in [('--list',), ()]:
            with self.subTest(flags=flags):
                result = self.run_mutation(base, *flags, status=2, diagnostic=diagnostic)
                self.assertEqual(result.returncode, 2, result)
                self.assertIn(diagnostic, result.stderr)
                self.assertIn('cargo mutants --list --json failed (exit 2)', result.stderr)
                self.assertNotIn('no generated Rust mutants', result.stderr)
                self.assertNotIn('Traceback', result.stderr)

    def test_mutation_rejects_invalid_list_output(self):
        base = self.mutation_step('src/lib.rs', 'pub fn f() {}\n', 'pub fn g() {}\n')
        for listing, message in [('invalid JSON', 'returned invalid JSON'), ('{}', 'expected a JSON array')]:
            with self.subTest(listing=listing):
                result = self.run_mutation(base, '--list', listing=listing)
                self.assertEqual(result.returncode, 1, result)
                self.assertIn(message, result.stderr)
                self.assertNotIn('no generated Rust mutants', result.stderr)
                self.assertNotIn('Traceback', result.stderr)


if __name__ == '__main__':
    unittest.main()
