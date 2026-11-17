#!/usr/bin/env python3
"""Executed defect witnesses for the fast checks and exact-revision push hook."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import checks

ROOT = Path(__file__).resolve().parent.parent
# Witnesses of known defects in the checks are skipped unless this is set: `BENNETT_RUN_DEFECTS=1`.
DEFECTS = os.environ.get('BENNETT_RUN_DEFECTS') == '1'


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

    @unittest.skipUnless(DEFECTS, 'defect: the bare-time check reads only .py/.sh/.js/.ts names, so a shebang script is never read')
    def test_bare_times_reads_extensionless_shell_scripts(self):
        # .githooks/pre-push is such a script in this tree; a literal there must be flagged like one in run.sh.
        (self.root / '.githooks').mkdir()
        hook = self.root / '.githooks/pre-push'
        hook.write_text('#!/bin/sh\nsleep 5\n')
        self.assertEqual(len(checks.bare_times(self.root, [Path('.githooks/pre-push')])), 1)

    @unittest.skipUnless(DEFECTS, 'defect: a positional Python timeout literal (wait(30), communicate(None, 30)) is not flagged')
    def test_bare_times_flags_positional_timeouts(self):
        # Popen.wait(timeout) and Popen.communicate(input, timeout) take the timeout positionally.
        (self.root / 'run.py').write_text('import subprocess\np = subprocess.Popen(["x"])\np.wait(30)\np.communicate(None, 30)\n')
        self.assertEqual(len(checks.bare_times(self.root, [Path('run.py')])), 2)

    @unittest.skipUnless(DEFECTS, 'defect: hygiene skips any file that is not UTF-8, host paths in it included')
    def test_hygiene_reads_files_that_are_not_utf8(self):
        # One Latin-1 byte must not hide a home path in the rest of a text file.
        (self.root / 'notes.txt').write_bytes(b'caf\xe9 /ho' b'me/example/private\n')
        self.assertIn('host-specific path', ' '.join(checks.hygiene(self.root, [Path('notes.txt')])))

    @unittest.skipUnless(DEFECTS, "defect: git's amend! subjects (commit --fixup=amend:) pass the no-WIP check on main")
    def test_amend_subject_is_rejected_on_main(self):
        base = self.init_repo()
        (self.root / 'note').write_text('amended')
        self.commit('amend! Accepted base')
        tip = self.git('rev-parse', 'HEAD')
        self.assertEqual(len(push.wip_commits(self.root, tip, base)), 1)

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


if __name__ == '__main__':
    unittest.main()
