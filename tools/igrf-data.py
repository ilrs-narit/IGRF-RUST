#!/usr/bin/env python3
"""Offline application-data backup. Backups are directories, never tar archives."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import sqlite3
import stat
import subprocess
import tempfile
import uuid


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def files(root):
    if root.is_symlink() or not root.is_dir():
        raise ValueError(f'Not a plain directory: {root}')
    result = {}
    for path in root.rglob('*'):
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
            continue
        if not stat.S_ISREG(mode):
            raise ValueError(f'Symlink or special file forbidden: {path}')
        result[path.relative_to(root).as_posix()] = path
    return result


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def recovery_marker(data):
    return data.parent / '.igrf-restore-in-progress'


def require_clean_restore(data):
    marker = recovery_marker(data)
    if marker.exists() or marker.is_symlink():
        raise ValueError(f'Interrupted restore requires administrator recovery: {marker}')


def flush_tree(root):
    for path in files(root).values():
        with path.open('rb') as stream:
            os.fsync(stream.fileno())
    for directory in sorted((p for p in root.rglob('*') if p.is_dir()),
                            key=lambda p: len(p.parts), reverse=True):
        sync_directory(directory)
    sync_directory(root)


def check_data(root):
    config = root / 'SystemConfig.json'
    if not isinstance(json.loads(config.read_text()), dict):
        raise ValueError('SystemConfig.json must contain a JSON object')
    database = root / 'tle_data.db'
    if database.exists():
        with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as db:
            if db.execute('PRAGMA integrity_check').fetchall() != [('ok',)]:
                raise ValueError('SQLite integrity check failed')


def backup(data, destination, version):
    data, destination = data.absolute(), destination.absolute()
    require_clean_restore(data)
    source = files(data)
    if destination.exists() or destination.is_symlink():
        raise ValueError('Backup destination must not already exist')
    if destination.resolve().is_relative_to(data.resolve()):
        raise ValueError('Backup must be outside application data')
    stage = Path(tempfile.mkdtemp(prefix='.igrf-backup-', dir=destination.parent))
    try:
        payload = stage / 'files'
        payload.mkdir()
        entries = {}
        for name, path in source.items():
            if name in ('tle_data.db-wal', 'tle_data.db-shm', 'tle_data.db-journal'):
                continue
            target = payload / name
            target.parent.mkdir(parents=True, exist_ok=True)
            if name == 'tle_data.db':
                with sqlite3.connect(path.as_uri() + '?mode=ro', uri=True) as src:
                    with sqlite3.connect(target) as dst:
                        src.backup(dst)
                        dst.execute('PRAGMA journal_mode=DELETE')
            else:
                shutil.copyfile(path, target)
            entries[name] = {'sha256': digest(target), 'size': target.stat().st_size,
                             'mode': stat.S_IMODE(path.stat().st_mode) & 0o777}
        check_data(payload)
        (stage / 'manifest.json').write_text(json.dumps({
            'format': 1, 'app_version': version, 'files': entries,
        }, indent=2) + '\n')
        flush_tree(stage)
        stage.rename(destination)
        sync_directory(destination.parent)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def validate(archive):
    actual = files(archive)
    manifest = json.loads((archive / 'manifest.json').read_text())
    if manifest.get('format') != 1 or not isinstance(manifest.get('app_version'), str):
        raise ValueError('Unsupported backup manifest')
    entries = manifest.get('files')
    if not isinstance(entries, dict) or 'SystemConfig.json' not in entries:
        raise ValueError('Missing application data manifest')
    expected = {'manifest.json'}
    for name, entry in entries.items():
        path = PurePosixPath(name)
        if not name or path.is_absolute() or '..' in path.parts or path.as_posix() != name:
            raise ValueError(f'Unsafe backup path: {name}')
        if name in ('tle_data.db-wal', 'tle_data.db-shm', 'tle_data.db-journal'):
            raise ValueError('Backup must contain a standalone SQLite snapshot')
        key = 'files/' + name
        expected.add(key)
        file = actual.get(key)
        if (file is None or not isinstance(entry, dict)
                or type(entry.get('mode')) is not int or not 0 <= entry['mode'] <= 0o777
                or file.stat().st_size != entry.get('size')
                or digest(file) != entry.get('sha256')):
            raise ValueError(f'Backup checksum/metadata mismatch: {name}')
    if set(actual) != expected:
        raise ValueError('Unlisted backup files')
    check_data(archive / 'files')
    return manifest


def restore(archive, data, owner=None):
    archive, data = archive.absolute(), data.absolute()
    require_clean_restore(data)
    manifest = validate(archive)  # No destination writes before full validation.
    if archive.resolve().is_relative_to(data.resolve()):
        raise ValueError('Backup must be outside restore destination')
    if data.is_symlink():
        raise ValueError('Restore target must not be a symlink')
    if data.exists():
        files(data)
    stage = Path(tempfile.mkdtemp(prefix='.igrf-restore-', dir=data.parent))
    previous = data.parent / ('.igrf-before-restore-' + uuid.uuid4().hex)
    marker = recovery_marker(data)
    try:
        for name, entry in manifest['files'].items():
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(archive / 'files' / name, target)
            target.chmod(entry['mode'])
        # Validate the staged copy too: media may fail/change during copying.
        for name, entry in manifest['files'].items():
            if digest(stage / name) != entry['sha256']:
                raise ValueError(f'Staged checksum mismatch: {name}')
        check_data(stage)
        if owner is not None:
            for path in [stage, *stage.rglob('*')]:
                os.chown(path, owner, owner)
        stage.chmod(0o700)
        flush_tree(stage)
        with marker.open('x') as stream:
            json.dump({'destination': str(data), 'staged': str(stage),
                       'previous': str(previous)}, stream)
            stream.flush()
            os.fsync(stream.fileno())
        sync_directory(data.parent)
        if data.exists():
            data.rename(previous)
        try:
            stage.rename(data)
        except OSError:
            if previous.exists():
                previous.rename(data)
                sync_directory(data.parent)
                marker.unlink()
                sync_directory(data.parent)
            raise
        sync_directory(data.parent)
        marker.unlink()
        sync_directory(data.parent)
        return previous if previous.exists() else None
    finally:
        # Keep recovery evidence and the staged payload after an interruption.
        if stage.exists() and not (marker.exists() or marker.is_symlink()):
            shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['backup', 'restore'])
    parser.add_argument('backup_directory', type=Path)
    parser.add_argument('--app-version', help='Exact deployed app version/commit (backup only)')
    parser.add_argument('--experiment-stopped', action='store_true', required=True,
                        help='Confirm normal experiment stop was completed before maintenance')
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error('Run as root on the appliance')
    if args.action == 'backup' and not args.app_version:
        parser.error('Backup requires --app-version')
    require_clean_restore(Path('/data/igrf'))
    if args.action == 'restore':
        validate(args.backup_directory.absolute())
    # Explicit stop suppresses Restart=; both remain stopped, including on error.
    subprocess.run(['systemctl', 'stop', 'igrf-app.service',
                    'igrf-x11-session.service'], check=True)
    for unit in ('igrf-app.service', 'igrf-x11-session.service'):
        state = subprocess.check_output(['systemctl', 'show', '--property=ActiveState',
                                         '--value', unit], text=True).strip()
        if state not in ('inactive', 'failed'):
            raise ValueError(f'Writer is not stopped: {unit} ({state})')
    data = Path('/data/igrf')
    if args.action == 'backup':
        backup(data, args.backup_directory, args.app_version)
    else:
        previous = restore(args.backup_directory, data, owner=1000)
        if previous:
            print(f'Previous data retained at {previous}')
    print('Complete. App and X11 remain stopped. No experiment was restarted.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as error:
        raise SystemExit(f'ERROR: {error}')
