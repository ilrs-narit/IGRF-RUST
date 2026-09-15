#!/usr/bin/env python3
"""Temporary-file integration checks; never invokes host systemd."""
import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('igrf_data', Path(__file__).with_name('igrf-data.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def refuses(call):
    try:
        call()
    except ValueError:
        return
    raise AssertionError('Invalid backup was accepted')


with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    data = root / 'source'
    data.mkdir()
    (data / 'SystemConfig.json').write_text('{"calibration": 42}')
    (data / '.env').write_text('EXAMPLE=value\n')
    (data / '.env').chmod(0o600)
    (data / 'logs').mkdir()
    (data / 'logs' / 'part.csv').write_text('time,value\n1,2\n')
    db = sqlite3.connect(data / 'tle_data.db')
    db.execute('PRAGMA journal_mode=WAL')
    db.execute('CREATE TABLE sample(value INTEGER)')
    db.execute('INSERT INTO sample VALUES(17)')
    db.commit()  # Keep connection open: committed row is still in WAL.
    archive = root / 'backup'
    m.backup(data, archive, 'test-commit')
    assert (data / 'tle_data.db-wal').stat().st_size > 0
    assert not (archive / 'files' / 'tle_data.db-wal').exists()
    target = root / 'fresh'
    m.restore(archive, target)
    with sqlite3.connect(target / 'tle_data.db') as restored:
        assert restored.execute('SELECT value FROM sample').fetchall() == [(17,)]
    assert (target / '.env').read_text() == 'EXAMPLE=value\n'
    assert (target / '.env').stat().st_mode & 0o777 == 0o600
    assert (target / 'logs' / 'part.csv').read_bytes() == (data / 'logs' / 'part.csv').read_bytes()
    old = m.restore(archive, target)
    assert old.is_dir() and (old / 'SystemConfig.json').exists()
    original = (target / 'SystemConfig.json').read_bytes()
    csv = archive / 'files' / 'logs' / 'part.csv'
    csv.write_text('corrupt')
    refuses(lambda: m.restore(archive, target))
    assert (target / 'SystemConfig.json').read_bytes() == original
    csv.write_bytes((data / 'logs' / 'part.csv').read_bytes())
    manifest_path = archive / 'manifest.json'
    manifest = json.loads(manifest_path.read_text())
    entry = manifest['files'].pop('.env')
    manifest['files']['../escape'] = entry
    manifest_path.write_text(json.dumps(manifest))
    refuses(lambda: m.restore(archive, target))
    manifest['files']['.env'] = manifest['files'].pop('../escape')
    manifest_path.write_text(json.dumps(manifest))
    csv.unlink()
    csv.symlink_to(data / 'logs' / 'part.csv')
    refuses(lambda: m.restore(archive, target))
    csv.unlink()
    csv.write_bytes((data / 'logs' / 'part.csv').read_bytes())
    (archive / 'files' / 'extra').write_text('unlisted')
    refuses(lambda: m.restore(archive, target))
    (data / 'link').symlink_to('/etc/passwd')
    refuses(lambda: m.backup(data, root / 'unsafe', 'test'))
    db.close()
    (archive / 'files' / 'extra').unlink()
    real_rename = Path.rename

    def interrupted_rename(path, destination):
        result = real_rename(path, destination)
        if path == target:
            raise SystemExit('simulated power failure after moving original data')
        return result

    with patch.object(Path, 'rename', interrupted_rename):
        try:
            m.restore(archive, target)
        except SystemExit:
            pass
        else:
            raise AssertionError('Fault was not reached')
    marker = m.recovery_marker(target)
    recovery = json.loads(marker.read_text())
    assert not target.exists()
    assert Path(recovery['previous']).is_dir()
    assert Path(recovery['staged']).is_dir()
    refuses(lambda: m.restore(archive, target))
    refuses(lambda: m.backup(data, root / 'after-fault', 'test'))
    # Simulate explicit administrator rollback, then clear the recovery marker.
    Path(recovery['previous']).rename(target)
    m.sync_directory(root)
    marker.unlink()
    m.sync_directory(root)
    (data / 'link').unlink()
    with patch.object(m.os, 'fsync', side_effect=OSError('simulated media failure')):
        try:
            m.backup(data, root / 'bad-media', 'test')
        except OSError:
            pass
        else:
            raise AssertionError('Flush failure reported success')
    assert not (root / 'bad-media').exists()
print('PASS: WAL snapshot, fresh/existing restore, retained data, modes, corruption/path/symlink rejection, durable recovery marker, sync failure')
