# CSV retention and reflash recovery (#25)

CSV history has **no automatic deletion** and no 1 GiB retention cap. The
volatile system journal has its own unrelated diagnostic limit.

## Application logging contract

These are implementation defaults to test, not measured capacity guarantees:

- Begin a new segment at a local date change or before a row would make a file
  exceed **256 MiB**. Include a monotonically increasing segment/session suffix
  so clock changes never overwrite history. Each segment contains its header;
  rotate between complete rows. This keeps individual files below FAT32's limit.
- Check available bytes on the selected filesystem before starting, rotating or
  switching destination, and every **5 seconds** using a monotonic timer. Warn
  below **1 GiB**. Stop logging and request the normal experiment stop below
  **256 MiB** to leave space for config/DB; these thresholds must be validated
  against measured write rates and other writers. Never delete files to recover
  space. Preflight checks do not replace checking every write/flush/sync error.
- Default destination is `/data/igrf/logs`. Inserting USB does not change it.
  The user selects a mounted FAT32/exFAT drive and explicitly chooses to record
  there. Validate the mount and available space, flush/sync and close the old
  file successfully, then create a new segment on that USB. Leave old microSD
  files intact. Do not silently fall back to a directory underneath a lost mount.
- Check write and flush errors immediately; sync to storage at least every
  **5 seconds**, on rotation, destination change and stop. If recording cannot
  continue (full disk, USB removal, permission/I/O failure), show a persistent
  alert and request the normal experiment stop. Never resume by itself.
  A failed stop command is not independent physical power isolation.
- Safe removal requires ending all writes, checking final sync/close, and a
  successful OS unmount. Failure must not be reported as safe removal.

Deletion is manual only. The app's "Saved log files" panel can delete a dated
log segment after its own confirmation dialog; it refuses the segment currently
being written and deletes nothing on its own. That is not a capacity rule: an
appliance that fills its filesystem still stops logging with an alert rather
than freeing space.

**App work still required:** `igrf-io/src/csv_logger.rs` currently rotates by
local date and flushes rows, but does not implement the size, space, sync or
selected-USB rules above. The application must handle logger errors with the
normal stop mechanism and persistent alert. This ticket changes no app source;
these policies are not claims that the UI or logger already enforces them.

## Backup and restore

Run these scripts on the appliance as root, after ending the experiment through
its normal Stop action. `--experiment-stopped` records that operator confirmation;
service termination alone is not an experiment stop or power cutoff. Tooling
stops `igrf-app.service` and `igrf-x11-session.service`, checks they stopped, and
leaves them stopped even after failure. Do not run other writers or manually
start either service during maintenance.

Keep the three tooling files together (`backup-data.sh`, `restore-data.sh`,
`igrf-data.py`); Python 3 with its SQLite module and systemd are required.
The destination must be a new directory on a mounted external drive with enough
space. Use the exact deployed application commit/version, from the image manifest:

```sh
sudo tools/backup-data.sh /media/usb/igrf-backup-20260915 \
  --app-version DEPLOYED_APP_COMMIT --experiment-stopped
# After writing the new microSD image, mounting that backup drive, and checking
# the new app supports this backup's config/database schema:
sudo tools/restore-data.sh /media/usb/igrf-backup-20260915 --experiment-stopped
```

Backup includes regular files beneath `/data/igrf`: `SystemConfig.json`, optional
`.env`, CSV logs, and app home state. It uses Python's SQLite online-backup API
for `tle_data.db`, including committed WAL content; it never copies WAL/journal
sidecars as the backup. JSON syntax and SQLite integrity are checked. The manifest
records the supplied app version, per-file SHA-256, byte sizes and permission bits.
Backup must finish successfully before reflashing. Logs already recorded directly
on other USB drives remain there and require separate preservation; they are not
part of the microSD backup.

Restore rejects symlinks, special files, traversal, extra/unlisted files and
checksum mismatches. It validates the whole backup before destination writes,
then stages and verifies another copy before replacing `/data/igrf`. Files become
owned by the image's app UID/GID 1000; recorded file modes are restored. An existing
directory is retained as `/data/.igrf-before-restore-<id>`, never automatically
deleted. There must be room for both copies. No database schema migration or
calibration-range validation is performed: verify compatibility before restore
and check calibration in the stopped application before use.

An interrupted restore leaves `/data/.igrf-restore-in-progress`, flushed before
the first directory rename. Startup initialization and new backup/restore attempts
must refuse to proceed while it exists. Its JSON records the staged, previous and
destination paths. From the maintenance console, inspect these paths, validate the
chosen dataset and either complete the replacement or move the retained original
back to `/data/igrf`. Flush that directory and `/data`, then remove the marker and
flush `/data` again. Do not merely delete the marker to unblock boot. The tool
clears it only after a durable successful replacement or completed rollback.
This is a recovery signal, not a transaction or guarantee against storage-device
failure. Keep coils unpowered during reflash/recovery. Backup
checksums detect corruption, not a maliciously replaced manifest; keep the backup
under administrator control. `.env` may contain credentials, so protect the drive
physically; FAT/exFAT does not enforce Unix permissions. Existing files above
FAT32's per-file limit require exFAT (the tool does not split historical files).

### Network and administrator identity

These tools intentionally operate on application data only. Before reflash,
separately preserve `/data/ssh` and `/data/NetworkManager` using a local/offline
maintenance console, with their consumers stopped. Restore those directories
before starting sshd/NetworkManager, with NetworkManager's two bind mounts
unmounted, preserving root ownership and restrictive modes (private SSH keys,
`authorized_keys` and network profile files: 0600). Do not stop NetworkManager
from the SSH connection being used for the copy. Without this separate step,
reprovision admin public keys and network credentials on the new image; SSH host
identity changes. Do not mistake an app-only backup for full appliance recovery.

## Evidence

`python3 tools/test-buildroot-backup.py` exercises a real committed WAL snapshot,
fresh and existing-data restore, retained original data, credential file modes,
checksum mismatch, traversal, symlink and unlisted-file rejection, an interruption
between directory renames, marker refusal and a failing fsync in temporary
directories. It never controls host services. Appliance service shutdown, actual
FAT/exFAT media, power-loss recovery, reflash/restore, and the logging policies
require hardware/application integration tests. No soak-test result is claimed.
