# IGRF kiosk — Buildroot external tree

This is the `BR2_EXTERNAL` tree for the Raspberry Pi 4 kiosk image that boots
straight into `igrf-app`. The design is in
[`docs/buildroot-design.md`](../docs/buildroot-design.md); the platform
decision is [ADR 0003](../docs/adr/0003-pi4-kiosk-platform.md) and the
read-only-OS split is
[ADR 0001](../docs/adr/0001-read-only-os-separate-persistent-data.md).

## Pinned release

| Thing | Pin |
| --- | --- |
| Buildroot | **2026.08** (stable, released 2026-09-04) |
| Buildroot commit | `d5180309b1b66ef3b8eaccca70ad69be8e0729a1` |
| Rust (via `rust-bin`) | **1.97.1** |
| Kernel tarball | `raspberrypi/linux` `21b410140c47ffab5668399f6f143c7d7b935c8b`, `bcm2711` defconfig |
| Onboard | upstream tag `1.4.4-1` (see `package/onboard/`) |

### Why 2026.08 and not the 2025.02 LTS

The LTS line (2025.02.18) ships Rust 1.82.0. The resolved crate set in
`Cargo.lock` requires Rust 1.95, so the LTS toolchain cannot build the app.
2026.08 ships Rust 1.97.1, which clears the MSRV. This is a version
comparison, not a compile proof — see "Not verified" below.

## Build

The authoritative build procedure — host prerequisites, the pinned checkout,
the `O=`/`BR2_DL_DIR=` invocation, the image location, and what remains
unproven — is [`docs/buildroot-build.md`](../docs/buildroot-build.md). It is
deliberately not copied here: two recipes drift apart, and this file owns the
pin and the layout, not the commands.

## Layout

```
external.desc                     BR2_EXTERNAL identity
Config.in                         sources every package Config.in
external.mk                       includes every package .mk
configs/
  igrf_raspberrypi4_defconfig     the board defconfig
board/igrf/raspberrypi4/
  genimage.cfg                    boot + rootfs + data partitions
  post-build.sh, post-image.sh    image assembly hooks
  config_4_64bit.txt, cmdline_4.txt  Raspberry Pi firmware config
  rootfs-overlay/                 files baked into the root filesystem
package/                          igrf-app, onboard and its missing deps
```

## Not verified

The tree was **built once for the earlier Raspberry Pi 5 configuration**; that
build was superseded by [ADR 0003](../docs/adr/0003-pi4-kiosk-platform.md) and
never booted. Its image hashes, partition layout and the checks run on them are
recorded in [`docs/buildroot-build.md`](../docs/buildroot-build.md). The
Raspberry Pi 4 configuration has **not been built or booted yet**, so nothing
about boot-time behaviour is proven until
[`docs/buildroot-test-plan.md`](../docs/buildroot-test-plan.md) P1 and later
pass on hardware.
