# Prototype verification plan (#26)

Status: **not executed on hardware**. No acceptance criterion is marked passed.
Use the [confirmed design](buildroot-design.md), [build procedure](buildroot-build.md),
[data layout](buildroot-data.md) and [logging/backup contract](buildroot-log-policy.md).
Keep coil power disconnected. Firmware currently retains the last command on
host loss; powered testing needs an independently designed power-cutoff system.

Record image checksum/source commit, card size, Pi revision, panel model and
connection, firmware/kernel versions, ambient conditions and timestamped results.
Follow this proof order; stop and resolve failures before advancing.

| Test | Acceptance | Procedure and pass evidence |
| --- | --- | --- |
| P0 Sources/configuration | Prerequisite | Verify Buildroot commit, every source/hash and resolved Kconfig. Build the image with captured logs; inspect all three partitions and actual ext4 `/data`. Confirm all requested defconfig symbols remain selected. |
| P1 Offline kiosk | 1 | Disconnect network, boot microSD. App opens stopped in fullscreen; verify X11/GPU rendering, 1024×600 mode, USB touch edges, Onboard typing and GTK file selection. Record the now-known panel model. Close/reopen dialogs during mock activity and check control timing separately. |
| P2 Persistent data and permissions | 2 | On cards of 32 GB and a larger size, confirm only partition 3 grows. `findmnt / /data` shows read-only SquashFS and writable ext4. As `igrf`, verify app data/home writes work and `/etc` writes fail. Save config/DB/log data, reboot, compare hashes and database records. Verify non-root UID and graphics/serial groups. |
| P3 Network and identity | 2 | Connect wired LAN and password Wi-Fi using nmcli, reboot, verify autoconnect and retained SSH host fingerprint. Confirm no second DHCP client. Disconnect internet and boot again; app still starts. Repeat using the Wi-Fi UI once implemented. Test DNS and instrument LAN routes independently. |
| P4 USB logging and safe eject | 3 | Test FAT32 and exFAT, then two simultaneous drives. Insertion appears in drive list without changing log target. Once app integration exists, explicitly choose a drive, close/sync old segment and start a new USB file; old SD files remain. Attempt eject with open files (must fail), then close and eject (must succeed). Check transferred hashes on another computer. |
| P5 Recording failures | 4 | In disposable test media/filesystems simulate low space, full disk, write/flush failure and USB unplug while recording. Require visible persistent error and normal stop request, no silent fallback directory and no automatic resume. Confirm data reserve thresholds using measured writer rates. Not executable end-to-end until app logging changes exist. |
| P6 Crash/retry | 5 | Kill app during unpowered mock activity. Restart must be stopped/disconnected. Repeated failures must exhaust 3 attempts within 300 seconds. Repeat with X11/session-bus failure. Service restart is not proof that coil power is cut. |
| P7 Time correctness | 6 | Boot offline with deliberately wrong clock and with/without RTC battery. Query sync state; ensure pending UI blocks timed work until operator verifies/sets time. Manual setting during timed work must be rejected. Reconnect NTP and inspect clock changes; active-run clock-step policy must be resolved before that scenario can pass. |
| P8 Corruption and reflash recovery | 7 | Corrupt config, require stopped recovery screen and a validated selected backup (UI pending). Create a database with committed WAL records; backup and verify those records survive restore. Reject modified checksum/path/symlink backups without changing live data. Reflash a separate card and restore app data, then separately restore network/SSH identity. Test interrupted partition growth, config save and restore-marker recovery independently. |
| P9 72-hour soak | 8 | After preceding tests and pending app work pass, run unpowered continuously for 72 hours. Track RSS, loop latency, storage free bytes, segment boundaries and timestamps; verify no missing/duplicate rows beyond documented failure tolerance. Exercise manual USB destination changes with hashes. |
| P10 30-day soak | 8 | Repeat for 30 days after the 72-hour gate. Archive the same metrics and complete log inventory. Run forced unplug/full-disk/recovery tests separately so failures are not hidden inside the soak result. |

Host tests provide narrower evidence: `tools/test-buildroot-{board,data,usb,backup}.py`
cover hook wiring, temporary-file partition growth, USB failure propagation and
real SQLite WAL backup/restore. They do not emulate a Pi, prove power-fail safety,
or replace any row above. Record tests as passed, failed or unverified with the
exact command, exit status and artifact/log paths. Never infer a hardware pass
from a successful host cargo build.
