# Buildroot kiosk build (#26)

## Recorded build

A full cross-build has been run on Arch Linux (x86_64, 14 CPUs, rsync 3.5.0,
wget 1.25.0, bc 1.08.2) against the pinned commit below, with the command in
"Commands" (`-j14`, no `local.mk`), and it completed: `sdcard.img` (491 MiB)
came out of `buildroot/output/images/`. Hashes of that build, partition layout,
image contents and the checks run against them are recorded
[below](#what-was-checked-on-that-build).

| Image | Bytes | SHA-256 |
| --- | --- | --- |
| `sdcard.img` | 514850816 | `cab0caef27f6d8ae5a973d403876f74211b580a762f893fdff587703a2793bb3` |
| `rootfs.squashfs` | 111083520 | `6d49b2850b2c8f65eb31a72cb9c451a9936459d4812e9101e314b002c4b63d7c` |
| `data.ext4` | 268435456 | `adf5d610bde6fd8731b62f34027faf251f952a4f0849c6aac28e9d60e0dda274` |
| `boot.vfat` | 134217728 | `bc08109fb0afa3c03132545e9d225e11c87f540db37627327d60c2e5ec0f1120` |

Pinning fixes versions, not bytes: a rebuild elsewhere is not expected to
reproduce these digests.

## What was checked on that build

- Partitions (`sfdisk`): 2048-264191 FAT32 bootable (128 MiB), 264192-481279
  Linux rootfs (106 MiB), 481280-1005567 Linux data (256 MiB); disk identifier
  `0x49475246`.
- `rootfs.squashfs`: SquashFS 4.0 (zlib), 10413 inodes, 10927 entries listed
  with Buildroot's own `unsquashfs`. It contains `usr/bin/igrf-app` (aarch64
  ELF, 13800224 bytes), `usr/bin/onboard`, the `gi/_gi_cairo` bridge,
  `usr/lib/systemd/system/igrf-app.service` (`User=igrf`), the `/usr/lib/igrf/`
  helpers, `SystemConfig.example.json`, `etc/X11/xinit/xinitrc` and the compiled
  dconf database.
- `/etc/passwd` in the image carries `igrf:x:1000:1000:...`. The users table is
  applied through the per-filesystem fakeroot script, so the identity is in the
  image and deliberately not in `output/target`.
- `data.ext4`: `e2fsck -fn` clean, label `data`, 12 files, and byte-identical to
  partition 3 of `sdcard.img` (`cmp` over the full 256 MiB).
- All 83 defconfig symbols are present in the resolved `.config`, and the Bootlin
  toolchain tarball's SHA-256 matches the pinned `.hash`.
- The image carries no updater tools: only what the packages and the overlay
  install.

Two failures were hit and fixed while producing it, both in this tree:
`python-gobject` built without pycairo, which Onboard's cairo bridge needs, and
`host-dconf` installed its systemd unit outside the host prefix.

## Not verified

No Raspberry Pi 5 boot has been run, so the HDMI/USB panel model, touch edges,
display mode, Wi-Fi firmware, boot time and the `/data` grow path are unproven,
as is Onboard's show/hide ownership (P1.3). The
[hardware test plan](buildroot-test-plan.md) still has no passing hardware
results, so acceptance criteria 1-8 of the design remain unmet. The image
supplies the OS contracts; new app controls and logging/recovery behaviour
listed below are not implemented by the platform tickets.

## Pins and prerequisites

| Component | Pin |
| --- | --- |
| Buildroot | 2026.08, `d5180309b1b66ef3b8eaccca70ad69be8e0729a1` |
| Host Rust supplied by Buildroot | rust-bin 1.97.1 |
| Raspberry Pi kernel | `21b410140c47ffab5668399f6f143c7d7b935c8b` |
| Raspberry Pi firmware | `063bcab6c8a90efb0d19f69d88cbbc7ec79cab68` (upstream recipe) |
| Onboard | `1.4.4-1` with downloaded source/license SHA-256 |
| dconf / hunspell / iso-codes / distutils-extra | Exact versions and hashes in `buildroot/package/*` |
| Application | Local source override; record the exact reviewed commit and `Cargo.lock` |

The [pinned upstream tree](https://gitlab.com/buildroot.org/buildroot/-/tree/d5180309b1b66ef3b8eaccca70ad69be8e0729a1)
is the source of the OS/toolchain values. LTS 2025.02.18 carries Rust 1.82.0,
below this crate set's MSRV. No app archive hash is fabricated for local mode.
Use a clean committed checkout for a releasable image: an unrecorded working
tree is not reproducible. Local mode fetches locked Cargo dependencies before
building; release archive mode and a vendored artifact hash remain future work.

Build as an ordinary user on Linux, with a case-sensitive filesystem and enough
free disk/RAM for a Rust/GTK toolchain. Install the mandatory packages from the
[Buildroot manual](https://buildroot.org/downloads/manual/manual.html#requirement-mandatory):
compiler/C++ compiler, make, binutils, bash, patch, gzip, bzip2, tar, cpio,
unzip, rsync, file, bc, findutils, sed, awk, diffutils, perl, Python 3, wget,
which and Git, plus the documented host development headers. Initial downloads
require internet. Record the host distribution/container digest and package
versions with each release; a container image digest has not yet been selected.

## Commands

From a clean IGRF repository checkout:

```sh
IGRF_REPO=$(pwd)
git clone https://gitlab.com/buildroot.org/buildroot.git buildroot-src
git -C buildroot-src checkout --detach d5180309b1b66ef3b8eaccca70ad69be8e0729a1
make -C buildroot-src O="$IGRF_REPO/buildroot/output" \
  BR2_EXTERNAL="$IGRF_REPO/buildroot" BR2_DL_DIR="$IGRF_REPO/buildroot/dl" \
  igrf_raspberrypi5_defconfig
printf 'IGRF_APP_OVERRIDE_SRCDIR = %s\n' "$IGRF_REPO" > buildroot/output/local.mk
make -C buildroot-src O="$IGRF_REPO/buildroot/output" \
  BR2_EXTERNAL="$IGRF_REPO/buildroot" BR2_DL_DIR="$IGRF_REPO/buildroot/dl"
```

Avoid spaces in the build/repository path (Buildroot restriction). `local.mk`
is optional — without it the app builds from this repository checkout — and
only exists to point the build at a different one. Keep
`buildroot/dl/` between builds; `output/` and `local.mk` are machine-local.
The resulting image should be `buildroot/output/images/sdcard.img`. Failures
in configuration, hash checks or compilation must be resolved before treating
this procedure as proven. Record `git rev-parse HEAD`, `git status --porcelain`,
the resolved output `.config`, host package/container versions, build log and
`sha256sum buildroot/output/images/*` alongside the image. Pinning means the
same source/dependency versions, not guaranteed byte-identical images.

Write the image to a card of 32 GB or larger. Raspberry Pi Imager's
custom-image option does this, and so does `dd`; either way the write destroys
whatever is on the card, so finish the
[backup procedure](buildroot-log-policy.md) first and identify the device
against the card you mean to overwrite:

```sh
lsblk -o NAME,SIZE,MODEL,MOUNTPOINT     # note the card; unplug and repeat if unsure
sudo dd if=buildroot/output/images/sdcard.img of=/dev/sdX bs=4M conv=fsync status=progress
sync
sudo partprobe /dev/sdX
lsblk -o NAME,SIZE /dev/sdX             # expect 128M vfat plus two Linux partitions
sudo cmp -n 1048576 buildroot/output/images/sdcard.img /dev/sdX && echo "write verified"
```

Confirming the first megabyte against the image catches a truncated or
half-written card before it is trusted. The `panel model` recorded in P1 is the
one thing here that cannot be recovered later.

The data-grow service expands partition 3 on boot. Keep coil power disconnected
through all initial image tests and recovery work.

## Operating contracts

- **Network (#22):** NetworkManager exclusively manages LAN/Wi-Fi with its
  internal DHCP client and wpa_supplicant D-Bus backend. Profiles/state bind to
  `/data`; DNS points at its `/run` file. Connectivity checks and wait-online
  services do not gate the app. A narrow polkit rule grants `igrf` network
  operations. The existing app has wired controls; Wi-Fi list/password controls
  still need implementation. Verify via `nmcli device status`,
  `nmcli device wifi list`, `nmcli --ask device wifi connect SSID`, and reboot.
- **USB (#23):** udev requests a systemd instance for USB FAT/exFAT. Buildroot
  lacks udisks2, so this uses a small validated helper and kernel mount/unmount.
  Mounts appear as `/run/media/igrf-sda1` etc., matching the app's scanner.
  Unmounted directories are root-only to prevent writing into a lost mount.
  Insertion never selects a log destination. After the app closes all files,
  run `/usr/lib/igrf/eject-usb.sh sda1`; success requires checked `syncfs` and
  ordinary unmount. Busy/I/O failures remain failures. UI integration is pending.
- **Time (#24):** query `timedatectl show -p NTPSynchronized -p NTP -p TimeUSec`.
  timesyncd starts without waiting for an NTP response; its last-known clock
  file persists under `/data/timesync`. This is a lower-bound time estimate,
  not elapsed-time tracking while powered off. No RTC battery is assumed.
  Before time-dependent work the UI must require a verified/set time when
  offline. Manual changes are allowed only with timed work stopped. No blanket
  time-setting permission is granted to `igrf`; the guarded UI/helper remains
  pending. Automatic clock steps during an active run are an **open policy
  item**, not solved by timesyncd configuration. Timezone defaults to UTC.
- **Recovery (#25):** installed tools live together under `/usr/lib/igrf/`.
  See [data layout](buildroot-data.md) and [backup/log policy](buildroot-log-policy.md).
  Admin SSH keys must be provisioned separately; no credential is shipped.

The source of network settings is the
[NetworkManager configuration manual](https://networkmanager.dev/docs/api/latest/NetworkManager.conf.html).
The time unit follows [systemd 258.7](https://github.com/systemd/systemd/blob/v258.7/units/systemd-timesyncd.service.in).
