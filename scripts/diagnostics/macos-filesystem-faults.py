#!/usr/bin/env python3
"""Explicit synthetic process-crash / ENOSPC checks; never operates on app data.

The caller creates and mounts a disposable <= 600 MiB image, then detaches it.
This script refuses to fill a directory on the host filesystem. The test binary
must contain the ignored model_registry::fault_tests::isolated_filesystem_child.
"""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import shutil
import sqlite3
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
ALLOWED = ROOT / '.cache/fs-fault-study'
PREFIX = 'SPEAKMORE_FS_FAULT '
TEST = 'model_registry::fault_tests::isolated_filesystem_child'


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def assert_root(path):
    assert path.is_absolute() and ALLOWED in path.parents
    assert path.resolve() == path
    assert all(not part.is_symlink() for part in [path, *path.parents])


class Child:
    def __init__(self, binary, root, action, phase=''):
        env = dict(os.environ, SPEAKMORE_FS_FAULT_ROOT=str(root),
                   SPEAKMORE_FS_FAULT_ACTION=action, SPEAKMORE_FS_FAULT_PHASE=phase)
        self.process = subprocess.Popen([str(binary), '--exact', TEST, '--ignored',
                                         '--nocapture', '--test-threads=1'], env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, text=True)
        self.lines, self.events = [], []
        self.queue = queue.Queue()
        def read():
            for line in self.process.stdout:
                self.lines.append(line)
                offset = line.find(PREFIX)
                if offset >= 0:
                    event = json.loads(line[offset + len(PREFIX):])
                    self.events.append(event)
                    self.queue.put(event)
            self.queue.put(None)
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def checkpoint(self, phase):
        deadline = time.monotonic() + 90
        while True:
            event = self.queue.get(timeout=max(0.01, deadline - time.monotonic()))
            assert event is not None, ''.join(self.lines)
            if event.get('checkpoint') == phase:
                return

    def finish(self, killed=False):
        try:
            code = self.process.wait(timeout=90)
            self.reader.join(timeout=5)
            assert not self.reader.is_alive()
            assert code == (-signal.SIGKILL if killed else 0), ''.join(self.lines)
            return {'exit_code': code, 'events': self.events, 'log': ''.join(self.lines)}
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait(timeout=10)
            self.process.stdin.close()
            self.process.stdout.close()

    def terminate_owned(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)


def logical_data(root):
    # Only called after the owned process exits. RW permits SQLite WAL recovery;
    # all diagnostic SQL is read-only. Never use immutable=1 or omit a hot WAL.
    with sqlite3.connect('file:' + str(root / 'data.sqlite3') + '?mode=rw', uri=True) as db:
        assert db.execute('PRAGMA integrity_check').fetchall() == [('ok',)]
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []
        tables = ('settings', 'shortcuts', 'dictionary_entries', 'transcriptions',
                  'usage_events', 'license', 'app_rules', 'providers')
        return {table: db.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall()
                for table in tables}


def fill_volume(volume):
    assert_root(volume)
    stat = volume.stat()
    assert stat.st_dev != volume.parent.stat().st_dev, 'Refuse host filesystem'
    filler = volume / '.speakmore-enospc-owned'
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    fd = os.open(str(filler), flags, 0o600)
    written, actual_errno = 0, None
    try:
        fs = os.fstatvfs(fd)
        capacity = fs.f_blocks * fs.f_frsize
        assert os.fstat(fd).st_dev == stat.st_dev
        assert 0 < capacity <= 600 * 1024 * 1024, 'Refuse unbounded volume'
        block = os.urandom(1024 * 1024)
        while written < 512 * 1024 * 1024:
            try:
                written += os.write(fd, block)
            except OSError as error:
                if error.errno != errno.ENOSPC:
                    raise
                actual_errno = error.errno
                break
        assert actual_errno == errno.ENOSPC, 'No real ENOSPC observed'
        return filler, {'bytes_written': written, 'errno': actual_errno,
                        'capacity_bytes': capacity, 'device': stat.st_dev}
    except BaseException:
        filler.unlink(missing_ok=True)
        raise
    finally:
        os.close(fd)


def main():
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument('--test-binary', required=True, type=Path)
    args.add_argument('--run-directory', required=True, type=Path)
    args.add_argument('--volume', required=True, type=Path)
    opts = args.parse_args()
    run, volume, binary = opts.run_directory.absolute(), opts.volume.absolute(), opts.test_binary.resolve()
    assert_root(run)
    assert_root(volume)
    assert binary.is_file() and ROOT / 'target' in binary.parents
    run.mkdir(exist_ok=False)
    # Freeze the executable before launching children: a concurrent Cargo build
    # must not make separate cases silently run different binaries.
    frozen = run / 'fixture-child'
    with binary.open('rb') as source, frozen.open('xb') as destination:
        shutil.copyfileobj(source, destination, 1024 * 1024)
    frozen.chmod(0o700)
    binary = frozen
    report = {'synthetic_only': True, 'binary_sha256': digest(binary), 'cases': [],
              'limits': ['No physical power loss or drive failure', 'Synthetic model bytes; no ASR quality claim',
                         'No user database, microphone, cloud or native UI accessed',
                         'Not an interruption inside SQLite migration or registration transaction']}
    try:
        for phase in ('copying', 'before-rename', 'before-registration', 'after-registration', 'enospc-copying'):
            case = volume / (run.name + '-' + phase) if phase.startswith('enospc') else run / phase
            case.mkdir(exist_ok=False)
            (case / '.speakmore-fault-fixture').write_bytes(b'synthetic-filesystem-check-v1\n')
            entry = {'phase': phase}
            report['cases'].append(entry)
            entry['seed'] = Child(binary, case, 'seed').finish()
            seeded = next(event for event in entry['seed']['events'] if event.get('seeded'))
            before = logical_data(case)
            protected = {str(p.relative_to(case)): digest(p) for p in case.rglob('*')
                         if p.is_file() and (case / 'sources' in p.parents or case / 'models' in p.parents)
                         and not p.name.endswith('.lock')}
            gate = 'copying' if phase.startswith('enospc') else phase
            child = Child(binary, case, 'import', gate)
            filler = None
            try:
                child.checkpoint(gate)
                if phase.startswith('enospc'):
                    filler, entry['full_volume'] = fill_volume(volume)
                    child.process.stdin.write('continue\n')
                    child.process.stdin.flush()
                    entry['interrupted'] = child.finish()
                    row = next(e for e in entry['interrupted']['events'] if e.get('done'))['snapshot']['models']
                    row = next(r for r in row if r['modelId'] == seeded['new_id'])
                    assert row['state'] == 'import_failed', row
                    assert row['error'] == 'InsufficientSpace', row
                    filler.unlink()
                    filler = None
                else:
                    child.process.kill()
                    entry['interrupted'] = child.finish(killed=True)
            finally:
                child.terminate_owned()
                if filler is not None:
                    filler.unlink()
            entry['reconcile'] = Child(binary, case, 'reconcile').finish()
            assert logical_data(case) == before, 'Protected logical data changed'
            assert all((case / path).is_file() and digest(case / path) == sha for path, sha in protected.items())
            assert not list((case / 'models/staging').glob('job-*'))
            rows = next(e for e in entry['reconcile']['events'] if e.get('done'))['snapshot']['models']
            expected = 'installed' if phase in ('before-registration', 'after-registration') else 'absent'
            assert next(row for row in rows if row['modelId'] == seeded['new_id'])['state'] == expected, rows
            if expected == 'absent':
                entry['retry'] = Child(binary, case, 'import').finish()
                rows = next(e for e in entry['retry']['events'] if e.get('done'))['snapshot']['models']
                assert next(row for row in rows if row['modelId'] == seeded['new_id'])['state'] == 'installed'
                assert logical_data(case) == before
            entry['passed'] = True
            print(json.dumps({'phase': phase, 'passed': True}), flush=True)
    finally:
        (run / 'report.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
