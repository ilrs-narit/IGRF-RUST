# Buildroot kiosk build (#26)

## Images from CI

Every push to `master` runs the `image` job in
`.github/workflows/release.yml`. It builds the Pi 4 image from that checkout
using the pinned Buildroot commit and caches source downloads only; the build
output is recreated for each run. The release waits for both application
builds and the image job to succeed.

Download `igrf-pi4-sdcard.img.gz` and its `.sha256` file from the GitHub Release
(or the `package-pi4-image` workflow artifact, retained for seven days).
Verify and decompress them in the same directory:

```sh
sha256sum -c igrf-pi4-sdcard.img.gz.sha256
gzip -dk igrf-pi4-sdcard.img.gz
```

Select the resulting `.img` when flashing the card yourself. CI never flashes
a device. Back up the card first: writing the full image replaces its data
partition too. A successful build does not establish hardware acceptance.

## Recorded build

A full cross-build has been run on Arch Linux (x86_64, 14 CPUs, rsync 3.5.0,
wget 1.25.0, bc 1.08.2) against the pinned commit below, with the command in
"Commands" (`-j14`, no `local.mk`), and it completed: `sdcard.img` (496 MiB)
came out of `buildroot/output/images/`. Hashes of that build, partition layout,
image contents and the checks run against them are recorded
[below](#what-was-checked-on-that-build).

The host session interrupted the build twice and the image was rebuilt after
each round of boot fixes below; the invocations share one log, and the recorded
images come from the last build, which exited 0. Resuming is safe at package
boundaries, but the interruption is part of the record, not something a
reproduction should copy. Earlier Raspberry Pi 5 records remain in this file's
git history.

| Image | Bytes | SHA-256 |
| --- | --- | --- |
| `sdcard.img` | 520093696 | `c7e9ebf10a60e6034ef80151f5b7a7be090b5fcf4d4c170774ee06e631019655` |
| `rootfs.squashfs` | 115511296 | `62c183dffa1a9e373e47175ac964b0ad95d2f480383c24bcd49619fc63dfabd1` |
| `data.ext4` | 268435456 | `6821a764bae67ac833b734c2155e8392dccf3754bc1c72a9df730c4b36c1e331` |
| `boot.vfat` | 134217728 | `4af66d786b64d7e61f7de00ed50804cc70434e5bc02af20754870917ffaeeb2c` |
| `Image` (kernel) | 24496640 | inside `boot.vfat` |

Pinning fixes versions, not bytes: a rebuild elsewhere is not expected to
reproduce these digests.

## What was checked on that build

- Partitions (`sfdisk`): 2048-264191 FAT32 bootable (128 MiB), 264192-491519
  Linux rootfs (111 MiB), 491520-1015807 Linux data (256 MiB); disk identifier
  `0x49475246`.
- `boot.vfat`: `Image` (24496640 bytes), `bcm2711-rpi-4-b.dtb` and the `-400`,
  `-cm4`, `-cm4s` device trees, **`start4.elf` (2303232 bytes) and `fixup4.dat`
  (5498 bytes)** — the firmware pair the Pi 4 boot ROM loads, which the earlier
  Pi 5 image deliberately did not carry — plus `config.txt` (533 bytes),
  `cmdline.txt` (137 bytes) and `overlays/` (358 dtbo files). `config.txt` and
  `cmdline.txt` in the image are byte-identical to the board files in this tree.
- `rootfs.squashfs`: SquashFS 4.0, gzip, 10128 inodes, 10642 entries listed with
  Buildroot's own `unsquashfs`. It contains `usr/bin/igrf-app`,
  `usr/bin/onboard`, the `gi/_gi_cairo` bridge, `igrf-app.service` with its
  `data.conf` drop-in, the `/usr/lib/igrf/` helpers, `SystemConfig.example.json`,
  `etc/X11/xinit/xinitrc`, the compiled dconf database, Onboard's data under
  `usr/share/onboard/` (nothing under `/share`), a compiled GSettings database
  that names Onboard's schema, and DejaVu fonts. No `__pycache__` under
  `/usr/lib/igrf`; the only bytecode caches are Python's own standard-library
  ones.
- `/etc/passwd` in the image carries
  `igrf:x:1000:1000:IGRF kiosk:/data/igrf:/usr/sbin/nologin`. The users table is
  applied through the per-filesystem fakeroot script, so the identity is in the
  image and deliberately not in `output/target`.
- `data.ext4`: `e2fsck -fn` clean, label `data`, 12 files, and byte-identical to
  partition 3 of `sdcard.img` (`cmp` over the full 256 MiB).
- All 81 symbols of `igrf_raspberrypi4_defconfig` are present in the resolved
  `.config`, and the Bootlin toolchain tarball's SHA-256 matches the pinned
  `.hash`.
- The image carries no updater tools: only what the packages and the overlay
  install.

Two build-time failures, both fixed in this tree, are still what makes this
build work: `python-gobject` built without pycairo, which Onboard's cairo bridge
needs, and `host-dconf` installed its systemd unit outside the host prefix. The
Pi 4 port itself built to exit 0 with no new failures.

The first boot on the Pi 4 then failed in two ways, both fixed afterwards and
both re-checked against the image's own binaries:

- `igrf-x11-session.service` exited 1: the launcher called `/bin/su`, which this
  image has only as a busybox applet that rejects the `-s`/`-c` form. The
  launcher now drops root with util-linux `setpriv` (`--reuid`, `--regid`,
  `--init-groups`), and `board/igrf/raspberrypi4/busybox.fragment` disables the
  busybox `setpriv` applet, which would otherwise shadow the real binary at
  install time and accepts no `--reuid`.
- `sshd.service` exited 1 *after* its key script succeeded: OpenSSH exits without
  a privilege-separation user, and `systemd-sysusers` cannot add one to a
  read-only root. `users.txt` now creates `sshd` at image build time, and the
  unit drop-in sets `RuntimeDirectory=sshd` for the directory sshd also needs.

The second boot reached the X server and failed there as well; three image
problems were found and fixed:

- `modesetting_drv.so` failed to load with `undefined symbol:
  gbm_bo_get_plane_count`, and the server exited with "No drivers available".
  The autotools build of xserver never links the modesetting driver against
  `$(GBM_LIBS)`, although the driver calls `gbm_bo_get_plane_count()` and the
  meson build lists `gbm_dep` for it. This tree patches it
  (`buildroot/patches/xserver_xorg-server/`) through a new
  `BR2_GLOBAL_PATCH_DIR` entry; `Makefile.in` is patched alongside
  `Makefile.am` because the build uses the tarball's shipped `configure`. The
  rebuilt module carries `NEEDED libgbm.so.1`, and loading it under
  `qemu-aarch64` registers the modesetting driver; the pre-fix module
  reproduces the boot error there.
- Onboard's `data_files` resolve against a "/" prefix in this cross build, so
  its data, D-Bus service file and GSettings schema landed in `/share`, and the
  schema XML never reached the staging directory from which libglib2 compiles
  the target's schema database at finalization. The package hook now merges
  `/share` into `/usr/share` and copies the schema XML into staging; the image's
  schema database contains Onboard's schema, where before it was absent.
- GTK and Onboard had no scalable font — the image carried only X11 bitmap
  fonts — so the image now selects DejaVu.

## Not verified

No Raspberry Pi 4 boot has reached the kiosk yet — the first boot stopped at the
console and the second at the X server, both sets of failures recorded above —
so the HDMI/USB panel model, touch edges, display mode, Wi-Fi firmware, boot
time and the `/data` grow path are unproven, as is Onboard's show/hide ownership
(P1.3). The
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
  igrf_raspberrypi4_defconfig
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

## Systemd time epoch (SCRUM-57)

systemd compiles `TIME_EPOCH` into the image as a lower bound for the system
clock. The Pi 4 has no RTC battery, so with no network the clock starts from
this value and the journal and log files are dated by it until timesyncd
first syncs.

systemd's meson resolves the value from the `-Dtime-epoch` option, then
`$SOURCE_DATE_EPOCH`, then the latest git tag, then the mtime of its `NEWS`
file. The systemd 258.7 tarball has no `.git` and neither the local build nor
the CI image job exports `SOURCE_DATE_EPOCH`, so the build fell through to
the `NEWS` mtime — the upstream release date 2026-03-13 — and the Pi booted
in March 2026. `buildroot/external.mk` therefore appends
`-Dtime-epoch=$(shell date +%s)` to `SYSTEMD_CONF_OPTS`, pinning the epoch to
the image build time. The override lives in the external tree's makefile, so
it covers local and CI builds alike.

The tradeoff is that the image is not reproducible: the epoch changes on
every build, so two builds of the same tree differ. `BR2_REPRODUCIBLE` stays
off.

The epoch is baked in when the systemd package is configured, so changing it
means reconfiguring that package — delete its build directory, then rebuild;
the same invocation regenerates `buildroot/output/images/sdcard.img`:

```sh
rm -rf buildroot/output/build/systemd-258.7
make -C buildroot-src O="$IGRF_REPO/buildroot/output" \
  BR2_EXTERNAL="$IGRF_REPO/buildroot" BR2_DL_DIR="$IGRF_REPO/buildroot/dl"
```

Verify the compiled value in the build tree; meson writes it to `config.h` in
the package build directory:

```sh
grep TIME_EPOCH buildroot/output/build/systemd-258.7/buildroot-build/config.h
```

Expect `#define TIME_EPOCH <seconds>` within the build window; check it with
`date -u -d @<seconds>`.

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
