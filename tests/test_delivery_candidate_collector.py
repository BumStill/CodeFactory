"""独立候选协议验收；不签发来源回执，不写真实工作区。"""
import dataclasses
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest import mock
import errno

from tools.delivery_candidate.collector import (CandidateError, Candidate, Limits, File, collect_archive,
    collect_process, conflict_plan)


def archive(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w', format=tarfile.USTAR_FORMAT) as tar:
        for name, data, kind in entries:
            entry = tarfile.TarInfo(name)
            entry.type = kind
            entry.mode = 0o755 if kind == tarfile.DIRTYPE else 0o644
            entry.size = len(data) if kind == tarfile.REGTYPE else 0
            entry.linkname = 'target' if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE) else ''
            tar.addfile(entry, io.BytesIO(data))
    return output.getvalue()


class CollectorTests(unittest.TestCase):
    def test_immutable_unadmitted_and_conflict_plan(self):
        result = collect_archive(archive([('new', b'new', tarfile.REGTYPE), ('kept', b'next', tarfile.REGTYPE)]))
        self.assertFalse(result.admitted)
        with self.assertRaises(dataclasses.FrozenInstanceError):
            result.digest = 'forged'
        baseline = {'kept': File(b'old', 0o644), 'deleted': File(b'delete', 0o644)}
        current = dict(baseline, foreign=File(b'foreign', 0o644))
        plan = conflict_plan(result, baseline, current)
        self.assertEqual(plan.changes, ('deleted', 'kept', 'new'))
        self.assertEqual(plan.conflicts, ())
        self.assertFalse(plan.apply_allowed)
        current['kept'] = File(b'concurrent', 0o644)
        self.assertEqual(conflict_plan(result, baseline, current).conflicts, ('kept',))
        self.assertEqual(current['foreign'].data, b'foreign')

    def test_conflict_plan_namespace_and_directory_changes(self):
        child = collect_archive(archive([('a/b', b'new', tarfile.REGTYPE)]))
        parent = collect_archive(archive([('a', b'new', tarfile.REGTYPE)]))
        directory = collect_archive(archive([('a', b'', tarfile.DIRTYPE)]))
        self.assertEqual(conflict_plan(child, {}, {'a': File(b'foreign', 0o644)}).conflicts, ('a/b',))
        self.assertEqual(conflict_plan(parent, {}, {'a/b': File(b'foreign', 0o644)}).conflicts, ('a',))
        self.assertEqual(conflict_plan(directory, {}, {'a': File(b'foreign', 0o644)}).conflicts, ('a',))
        self.assertEqual(conflict_plan(child, {}, {'A/B': File(b'foreign', 0o644)}).conflicts, ('a/b',))
        with self.assertRaises(CandidateError):
            conflict_plan(child, {}, {'A': File(b'1', 0o644), 'a': File(b'2', 0o644)})
        with self.assertRaises(CandidateError):
            collect_archive(archive([('A', b'', tarfile.DIRTYPE), ('a/b', b'x', tarfile.REGTYPE)]))

    def test_forged_candidates_snapshot_budgets_and_modes(self):
        good = collect_archive(archive([('x', b'ok', tarfile.REGTYPE)]))
        for files, dirs, digest in [(list(good.files), (), good.digest), (good.files, [], good.digest), (good.files, (), 'forged')]:
            with self.assertRaises(CandidateError):
                Candidate(files, dirs, digest)
        with self.assertRaises(CandidateError):
            conflict_plan(good, {}, {str(i): File(b'x', 0o644) for i in range(65)})
        with self.assertRaises(CandidateError):
            conflict_plan(good, {}, {'x': File(b'x' * (2 * 1024 * 1024 + 1), 0o644)})
        content = io.BytesIO()
        with tarfile.open(fileobj=content, mode='w', format=tarfile.USTAR_FORMAT) as tar:
            entry = tarfile.TarInfo('private'); entry.mode=0o600; entry.size=1
            tar.addfile(entry, io.BytesIO(b'x'))
        with self.assertRaises(CandidateError):
            collect_archive(content.getvalue())
        child = collect_archive(archive([('a/y', b'new', tarfile.REGTYPE)]))
        self.assertEqual(conflict_plan(child, {}, {'A/x': File(b'foreign', 0o644)}).conflicts, ('a/y',))

    def test_darwin_zombie_group_eperm_does_not_break_normal_output(self):
        real_killpg = os.killpg
        def darwin_killpg(pid, sig):
            exited = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            if exited is not None:
                raise PermissionError(errno.EPERM, 'simulated Darwin zombie-only group')
            return real_killpg(pid, sig)
        with tempfile.TemporaryDirectory() as root:
            path = Path(root)/'payload'; path.write_bytes(archive([('ok', b'ok', tarfile.REGTYPE)]))
            with mock.patch('tools.delivery_candidate.collector.os.killpg', side_effect=darwin_killpg):
                result = collect_process([sys.executable, '-c', 'import sys;sys.stdout.buffer.write(open(sys.argv[1],"rb").read())', str(path)])
            self.assertEqual(dict(result.files)['ok'].data, b'ok')

    def test_kill_error_preserves_primary_and_closes_reaps(self):
        real_popen = subprocess.Popen
        processes = []
        def tracked(*args, **kwargs):
            proc = real_popen(*args, **kwargs); processes.append(proc); return proc
        with mock.patch('tools.delivery_candidate.collector.subprocess.Popen', side_effect=tracked), mock.patch('tools.delivery_candidate.collector.os.killpg', side_effect=PermissionError(errno.EPERM, 'injected')):
            with self.assertRaisesRegex(CandidateError, 'EXPORT_EXIT_NONZERO') as failure:
                collect_process([sys.executable, '-c', 'raise SystemExit(4)'])
        self.assertTrue(any('CLEANUP_UNCONFIRMED' in n for n in failure.exception.__notes__))
        self.assertTrue(all(p.stdout.closed and p.returncode is not None for p in processes))

    def test_timeout_with_kill_denial_still_reaps_direct_leader(self):
        real_popen = subprocess.Popen
        processes = []
        def tracked(*args, **kwargs):
            proc = real_popen(*args, **kwargs); processes.append(proc); return proc
        with mock.patch('tools.delivery_candidate.collector.subprocess.Popen', side_effect=tracked), mock.patch('tools.delivery_candidate.collector.os.killpg', side_effect=PermissionError(errno.EPERM, 'injected')):
            with self.assertRaisesRegex(CandidateError, 'COLLECT_TIMEOUT') as failure:
                collect_process([sys.executable, '-c', 'import time;time.sleep(0.3)'], timeout=0.1)
        self.assertTrue(any('CLEANUP_UNCONFIRMED' in n for n in failure.exception.__notes__))
        self.assertTrue(all(p.stdout.closed and p.stdin.closed and p.returncode is not None for p in processes))
        # Group denial leaves descendant cleanup unproven. This finite synthetic
        # exporter exits by itself; do not claim the supervisor killed it.
        time.sleep(0.4)

    def test_supervisor_status_missing_malformed_and_launch_failure(self):
        for source in ('import os;os.close(1)',
                       'import os,sys;os.write(int(sys.argv[1]),b"bad-status");os.close(1)',
                       'import os,sys;os.write(int(sys.argv[1]),b"x"*65);os.close(1)'):
            with mock.patch('tools.delivery_candidate.collector._EXPORT_SUPERVISOR', source), self.assertRaises(CandidateError):
                collect_process([sys.executable, '-c', 'pass'])
        with self.assertRaisesRegex(CandidateError, 'EXPORT_LAUNCH_FAILED'):
            collect_process(['/definitely-not-a-codefactory-exporter'])

    def test_kill_error_never_returns_candidate(self):
        real_popen = subprocess.Popen
        processes = []
        def tracked(*args, **kwargs):
            proc = real_popen(*args, **kwargs); processes.append(proc); return proc
        with tempfile.TemporaryDirectory() as root:
            path = Path(root)/'payload'; path.write_bytes(archive([('ok', b'ok', tarfile.REGTYPE)]))
            with mock.patch('tools.delivery_candidate.collector.subprocess.Popen', side_effect=tracked), mock.patch('tools.delivery_candidate.collector.os.killpg', side_effect=RuntimeError('injected unexpected kill exception')):
                with self.assertRaisesRegex(CandidateError, 'CLEANUP_UNCONFIRMED'):
                    collect_process([sys.executable, '-c', 'import sys;sys.stdout.buffer.write(open(sys.argv[1],"rb").read())', str(path)])
        self.assertTrue(all(p.stdout.closed and p.returncode is not None for p in processes))

    @unittest.skipUnless(os.name == 'posix', 'POSIX process-group adapter only')
    def test_success_cleans_delayed_child_with_closed_stdout(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root); marker = root/'late'; payload=root/'input.tar'
            payload.write_bytes(archive([('x', b'ok', tarfile.REGTYPE)]))
            child = f'import os,time,pathlib; os.close(1); time.sleep(0.6); pathlib.Path({str(marker)!r}).write_text("late")'
            script = f'import subprocess,sys;subprocess.Popen([sys.executable,"-c",{child!r}]);sys.stdout.buffer.write(open({str(payload)!r},"rb").read())'
            result = collect_process([sys.executable, '-c', script])
            self.assertEqual(dict(result.files)['x'].data, b'ok')
            time.sleep(0.8)
            self.assertFalse(marker.exists())

    def test_alias_traversal_links_special_and_duplicates(self):
        for name in ('../x', '/x', './x', 'a/../x', 'a//b', 'a/./b', 'C:/x', 'a\\b'):
            with self.subTest(name=name), self.assertRaises(CandidateError):
                collect_archive(archive([(name, b'x', tarfile.REGTYPE)]))
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE, tarfile.CHRTYPE):
            with self.subTest(kind=kind), self.assertRaises(CandidateError):
                collect_archive(archive([('x', b'', kind)]))
        for names in [('x', 'x'), ('X', 'x'), ('a', 'a/b')]:
            with self.subTest(names=names), self.assertRaises(CandidateError):
                collect_archive(archive([(n, b'x', tarfile.REGTYPE) for n in names]))

    def test_directory_depth_bytes_and_archive_budgets(self):
        cases = [(archive([(f'd{i}', b'', tarfile.DIRTYPE) for i in range(3)]), Limits(entries=2)),
                 (archive([('a/b/c', b'x', tarfile.REGTYPE)]), Limits(depth=2)),
                 (archive([('x', b'123', tarfile.REGTYPE)]), Limits(file_bytes=2)),
                 (archive([('x', b'12', tarfile.REGTYPE), ('y', b'12', tarfile.REGTYPE)]), Limits(total_bytes=3)),
                 (archive([('x', b'x', tarfile.REGTYPE)]), Limits(archive_bytes=100))]
        for content, limits in cases:
            with self.subTest(limits=limits), self.assertRaises(CandidateError):
                collect_archive(content, limits)

    def test_corruption_truncation_and_trailing_payload(self):
        good = archive([('x', b'value', tarfile.REGTYPE)])
        corrupted = bytearray(good); corrupted[0] ^= 1
        for raw in (bytes(corrupted), good[:513], good + b'payload'):
            with self.assertRaises(CandidateError):
                collect_archive(raw)

    @unittest.skipUnless(os.name == 'posix', 'POSIX process-group adapter only')
    def test_timeout_kills_group_and_delayed_writer(self):
        with tempfile.TemporaryDirectory() as root:
            marker = Path(root) / 'late'
            child = f'import time,pathlib; time.sleep(0.6); pathlib.Path({str(marker)!r}).write_text("late")'
            command = [sys.executable, '-c', f'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c",{child!r}]); time.sleep(20)']
            start = time.monotonic()
            with self.assertRaisesRegex(CandidateError, 'TIMEOUT'):
                collect_process(command, timeout=0.15)
            self.assertLess(time.monotonic() - start, 2)
            time.sleep(0.8)
            self.assertFalse(marker.exists(), 'observe beyond delayed child write window')

    @unittest.skipUnless(os.name == 'posix', 'POSIX process-group adapter only')
    def test_output_budget_nonzero_and_inherited_pipe(self):
        for source, error in [('import sys; sys.stdout.buffer.write(b"x"*200000)', 'BUDGET'),
                              ('raise SystemExit(4)', 'EXIT'),
                              ('import subprocess,sys; subprocess.Popen([sys.executable,"-c","import time; time.sleep(10)"])', 'TIMEOUT')]:
            with self.subTest(source=source), self.assertRaisesRegex(CandidateError, error):
                collect_process([sys.executable, '-c', source], timeout=0.2, limits=Limits(archive_bytes=4096))

    @unittest.skipUnless(os.name == 'posix', 'POSIX process-group adapter only')
    def test_real_generators_compiler_and_tar_stream(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            subprocess.run([sys.executable, '-c', 'from pathlib import Path; Path("python.txt").write_text("python-generated")'], cwd=root, check=True)
            if not shutil.which('node') or not shutil.which('cc'):
                self.skipTest('node and cc required for this normal-toolchain positive')
            subprocess.run(['node', '-e', 'require("fs").writeFileSync("node.txt", "node-generated")'], cwd=root, check=True)
            (root/'program.c').write_text('#include <stdio.h>\nint main(void){puts("compiler-generated");return 0;}')
            subprocess.run(['cc', 'program.c', '-o', 'program'], cwd=root, check=True)
            with (root/'compiled.txt').open('wb') as out:
                subprocess.run([str(root/'program')], stdout=out, check=True)
            payload = archive([(name, (root/name).read_bytes(), tarfile.REGTYPE) for name in ('python.txt', 'node.txt', 'program.c', 'program', 'compiled.txt')])
            path = root/'candidate.tar'; path.write_bytes(payload)
            candidate = collect_process([sys.executable, '-c', 'import sys;sys.stdout.buffer.write(open(sys.argv[1],"rb").read())', str(path)])
            files = dict(candidate.files)
            self.assertEqual(files['python.txt'].data, b'python-generated')
            self.assertEqual(files['node.txt'].data, b'node-generated')
            self.assertEqual(files['compiled.txt'].data, b'compiler-generated\n')
            self.assertFalse(candidate.admitted)


if __name__ == '__main__':
    unittest.main()
