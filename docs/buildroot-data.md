# Persistent data integration (#17, #21)

The image has three MBR partitions: 128 MiB boot FAT, the generated read-only
SquashFS, and an initially empty 256 MiB ext4 `data` filesystem. The latter is
an image-size floor, not a log quota. No history is deleted automatically.

`data.mount` requires `igrf-grow-data.service`. Before mounting, the script
checks that root is partition 2 on the microSD, that the MBR has the IGRF disk
ID and three expected partition types, and that partition 3 is an unmounted
ext4 filesystem labelled `data`. It grows only partition 3, preserves its
start, verifies readback, updates the kernel partition mapping, checks ext4,
and runs `resize2fs`. Failure prevents mounting and starting the app. Repeating
the sequence after a reboot retries filesystem growth even if partition growth
previously succeeded. It never formats an existing filesystem.

The growth operations use [sfdisk](https://man7.org/linux/man-pages/man8/sfdisk.8.html)
and [partx](https://man7.org/linux/man-pages/man8/partx.8.html). Updating a live
microSD partition map and interrupted growth still require hardware tests;
this is not a guarantee against power-loss damage to the MBR or filesystem.

The build users table creates `igrf` (UID/GID 1000, locked password) before
SquashFS is created, with video/render and serial dialout group access.
The matching sysusers entry records that identity;
runtime sysusers cannot create a missing account on the read-only `/etc`.
Tmpfiles prepares the directories after mounting, then configuration seeding
uses a flushed temporary file and an exclusive hard link. An existing config,
including a damaged one, is never replaced. The app's recovery UI is separate
work; seeding does not implement recovery or validate calibration values.

| State | Persistent location | Consumer |
| --- | --- | --- |
| Config, SQLite DB/WAL, CSV logs, app home | `/data/igrf` | App and X11 session wait for initialization |
| Remembered network profiles | `/data/NetworkManager/system-connections` | Bind mounted over `/etc/NetworkManager/system-connections` |
| NetworkManager runtime persistence | `/data/NetworkManager/state` | Bind mounted over `/var/lib/NetworkManager` |
| SSH host key and admin public keys | `/data/ssh` | sshd uses the image's dedicated configuration |

NetworkManager's two mounts are requested by its service after initialization;
they are not early local-fs dependencies (which would create an ordering
cycle). Wi-Fi UI, NetworkManager permissions and resolver policy remain #22.

SSH allows only administrator public-key login as root. Provision
`/data/ssh/authorized_keys` as root, mode 0600, before expecting remote access;
no public key is included in the image. Host keys are generated only when
absent and retained across boots. A damaged existing key requires recovery.

CSV logs persist. The diagnostic system journal is deliberately volatile,
limited to 32 MiB; export `journalctl -b` before reboot when troubleshooting.
Backing up only the main SQLite file while the app runs is not supported;
consistent snapshot and restore tooling belongs to #25.

## Verification

Run `python3 tools/test-buildroot-board.py` and
`python3 tools/test-buildroot-data.py` on Linux with util-linux `sfdisk`.
The latter uses temporary regular files and verifies partition growth,
unchanged earlier partitions and data bytes, repeated execution, refusal of
shrinking/foreign images, and configuration preservation. It does not touch
real disks or exercise kernel partition refresh and filesystem resizing.

The mount/init units pass host `systemd-analyze verify`. A Buildroot cross-build,
Pi boot, remembered Wi-Fi/SSH reboot checks, interrupted resize, and the
72-hour/30-day acceptance tests have **not** been run. Image reflashing still
requires backup and restore; persistence across reboot does not preserve data
when the whole card is overwritten.
