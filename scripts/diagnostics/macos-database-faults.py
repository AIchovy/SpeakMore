#!/usr/bin/env python3
"""Synthetic SQLite disk-full and post-commit crash checks on a bounded image.

Uses the real Store and normal migrations through an explicit cfg(test) child.
Requires a disposable mounted <=600 MiB volume under .cache/fs-fault-study.
"""
import argparse
from contextlib import closing
import errno
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3

spec = importlib.util.spec_from_file_location('filesystem_faults', Path(__file__).with_name('macos-filesystem-faults.py'))
fs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fs)
CANDIDATE = '00000000-0000-0000-0000-000000000002'
TABLES = ('settings', 'models', 'providers', 'shortcuts', 'app_rules', 'dictionary_entries',
          'transcriptions', 'usage_events', 'license', 'provider_credentials_pending')


def snapshot(root, omit_candidate=False):
    # Child must already have exited; preserve and read any hot WAL through SQLite.
    with closing(sqlite3.connect((root / 'data.sqlite3').as_uri() + '?mode=rw', uri=True)) as db:
        assert db.execute('PRAGMA integrity_check').fetchall() == [('ok',)]
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []
        result = {'version': db.execute('PRAGMA user_version').fetchone()[0], 'tables': {}}
        for table in TABLES:
            cols = [row[1] for row in db.execute('PRAGMA table_info(' + table + ')')]
            # The v7 migration only appends this nullable field. Compare every old column.
            selected = [col for col in cols if col != 'polish_reason']
            sql = 'SELECT ' + ','.join('"' + col + '"' for col in selected) + ' FROM ' + table
            args = ()
            if omit_candidate and table in ('transcriptions', 'usage_events'):
                sql += ' WHERE session_id != ?'
                args = (CANDIDATE,)
            digest, count = hashlib.sha256(), 0
            for row in db.execute(sql + ' ORDER BY 1', args):
                digest.update((json.dumps(row, ensure_ascii=False, separators=(',', ':'))+'\n').encode())
                count += 1
            result['tables'][table] = {'columns': selected, 'count': count, 'sha256': digest.hexdigest()}
            if table == 'transcriptions' and 'polish_reason' in cols:
                assert db.execute('SELECT count(*) FROM transcriptions WHERE polish_reason IS NOT NULL').fetchone()[0] == 0
        return result


def fill_tightly(volume, reserve=0):
    # Reserve a separate allocated file before filling. APFS may reject even
    # shrinking a large file when metadata space is exhausted; deleting this
    # one owned reserve file releases space without rewriting its extent tree.
    filler, reserve_file = None, None
    try:
        if reserve:
            candidate = volume / '.speakmore-database-reserve-owned'
            fd = os.open(str(candidate), os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            reserve_file = candidate
            try:
                remaining = reserve
                while remaining:
                    remaining -= os.write(fd, os.urandom(min(remaining, 1024*1024)))
                os.fsync(fd)
            finally:
                os.close(fd)
        filler, report = fs.fill_volume(volume)
        fd = os.open(str(filler), os.O_WRONLY | os.O_APPEND | os.O_NOFOLLOW)
        try:
            assert os.fstat(fd).st_dev == volume.stat().st_dev
            block = os.urandom(4096)
            extra = 0
            while extra < 2 * 1024 * 1024:
                try:
                    n = os.write(fd, block)
                    assert n > 0
                    extra += n
                except OSError as error:
                    assert error.errno == errno.ENOSPC
                    report['page_allocation_errno'] = error.errno
                    break
            assert report.get('page_allocation_errno') == errno.ENOSPC
            report['bytes_written'] += extra
            try:
                os.fsync(fd)
            except OSError as error:
                assert error.errno == errno.ENOSPC
                report['sync_errno'] = error.errno
        finally:
            os.close(fd)
        if reserve_file is not None:
            reserve_file.unlink()
            reserve_file = None
        report['released_before_operation'] = reserve
        state = os.statvfs(volume)
        report['available_before_operation'] = state.f_bavail * state.f_frsize
        return filler, report
    except BaseException:
        if filler is not None:
            filler.unlink(missing_ok=True)
        raise
    finally:
        if reserve_file is not None:
            reserve_file.unlink(missing_ok=True)


def event(result, key):
    return next(row for row in result['events'] if key in row)


def run_child(binary, case, action):
    return fs.Child(binary, case, action).finish()


def prepare_case(volume, run, name):
    case = volume / (run.name + '-' + name)
    case.mkdir(exist_ok=False)
    (case / '.speakmore-fault-fixture').write_bytes(b'synthetic-filesystem-check-v1\n')
    return case


def fault_run(binary, case, action, phase, volume, reserve=0, kill=False):
    child, filler = fs.Child(binary, case, action, phase), None
    try:
        child.checkpoint(phase)
        if kill:
            child.process.kill()
            return {'child': child.finish(killed=True)}
        filler, full = fill_tightly(volume, reserve)
        child.process.stdin.write('continue\n')
        child.process.stdin.flush()
        return {'volume': full, 'child': child.finish()}
    finally:
        child.terminate_owned()
        if filler is not None:
            filler.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-binary', type=Path, required=True)
    parser.add_argument('--run-directory', type=Path, required=True)
    parser.add_argument('--volume', type=Path, required=True)
    opts = parser.parse_args()
    run, volume, source = opts.run_directory.absolute(), opts.volume.absolute(), opts.test_binary.resolve()
    fs.assert_root(run)
    fs.assert_root(volume)
    assert source.is_file() and fs.ROOT / 'target' in source.parents
    assert volume.stat().st_dev != volume.parent.stat().st_dev
    vfs = os.statvfs(volume)
    assert vfs.f_blocks * vfs.f_frsize <= 600 * 1024 * 1024
    run.mkdir(exist_ok=False)
    binary = run / 'fixture-child'
    with source.open('rb') as src, binary.open('xb') as dst:
        shutil.copyfileobj(src, dst, 1024 * 1024)
    binary.chmod(0o700)
    report = {'synthetic_only': True, 'binary_sha256': fs.digest(binary), 'cases': [],
              'limits': ['No user database or microphone accessed', 'No physical power loss or drive failure',
                         'SIGKILL occurs after Store reports a committed write, not inside a transaction',
                         'No disk-full injection after a successful backup or during migration commit']}
    try:
        case = prepare_case(volume, run, 'history-full')
        item = {'name': 'history-full'}
        report['cases'].append(item)
        item['seed'] = run_child(binary, case, 'db-seed-current')
        before = snapshot(case)
        item['fault'] = fault_run(binary, case, 'db-write-history', 'before-write', volume)
        failed = event(item['fault']['child'], 'write_ok')
        assert failed['write_ok'] is False and failed['error']['code'] == 'StorageUnavailable', failed
        assert snapshot(case) == before, 'Partial history or usage commit'
        item['failed_write_preserved'] = before
        item['retry'] = run_child(binary, case, 'db-write-history')
        assert event(item['retry'], 'write_ok') == {'write_ok': True, 'new_history': True}
        after = snapshot(case)
        assert snapshot(case, omit_candidate=True) == before
        assert after['tables']['transcriptions']['count'] == before['tables']['transcriptions']['count'] + 1
        assert after['tables']['usage_events']['count'] == before['tables']['usage_events']['count'] + 1
        item['duplicate'] = run_child(binary, case, 'db-write-history')
        assert event(item['duplicate'], 'write_ok') == {'write_ok': True, 'new_history': False}
        assert snapshot(case) == after
        item['passed'] = True
        print(json.dumps({'case': item['name'], 'passed': True}), flush=True)

        case = prepare_case(volume, run, 'committed-kill')
        item = {'name': 'committed-kill'}
        report['cases'].append(item)
        item['seed'] = run_child(binary, case, 'db-seed-current')
        before = snapshot(case)
        item['fault'] = fault_run(binary, case, 'db-write-history', 'after-write', volume, kill=True)
        assert event(item['fault']['child'], 'write_ok') == {'write_ok': True, 'new_history': True}
        wal = case / 'data.sqlite3-wal'
        assert wal.is_file() and wal.stat().st_size > 0
        item['uncheckpointed_wal'] = {'bytes': wal.stat().st_size, 'sha256': fs.digest(wal)}
        after = snapshot(case)
        assert snapshot(case, omit_candidate=True) == before
        assert after['tables']['transcriptions']['count'] == 2
        assert after['tables']['usage_events']['count'] == 2
        item['retry'] = run_child(binary, case, 'db-write-history')
        assert event(item['retry'], 'write_ok') == {'write_ok': True, 'new_history': False}
        assert snapshot(case) == after
        item['passed'] = True
        print(json.dumps({'case': item['name'], 'passed': True}), flush=True)

        case = prepare_case(volume, run, 'backup-full')
        item = {'name': 'backup-full'}
        report['cases'].append(item)
        item['seed'] = run_child(binary, case, 'db-seed-v6')
        before = snapshot(case)
        assert before['version'] == 6
        recovery = case / 'data.sqlite3.recovery'
        backup = recovery / 'before-v6-to-v7.sqlite3'
        prior_hash = fs.digest(backup)
        item['source_bytes'] = (case / 'data.sqlite3').stat().st_size
        item['prior_backup'] = {'bytes': backup.stat().st_size, 'sha256': prior_hash}
        item['fault'] = fault_run(binary, case, 'db-open', 'before-open', volume, reserve=2*1024*1024)
        failed = event(item['fault']['child'], 'opened')
        assert failed['opened'] is False and failed['error']['code'] == 'DatabaseBackupFailed', failed
        assert snapshot(case) == before
        assert fs.digest(backup) == prior_hash
        assert (recovery / 'keep-unknown.txt').read_bytes() == b'preserve recovery note'
        item['failed_upgrade_preserved'] = before
        item['retry'] = run_child(binary, case, 'db-open')
        assert event(item['retry'], 'opened')['opened'] is True
        after = snapshot(case)
        assert after['version'] == 7 and after['tables'] == before['tables']
        assert not backup.exists()
        assert not list(recovery.glob('pending-*'))
        assert (recovery / 'keep-unknown.txt').read_bytes() == b'preserve recovery note'
        item['passed'] = True
        print(json.dumps({'case': item['name'], 'passed': True}), flush=True)
    finally:
        (run / 'report.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
