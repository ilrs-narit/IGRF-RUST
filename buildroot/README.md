# IGRF kiosk — Buildroot external tree

This is the `BR2_EXTERNAL` tree for the Raspberry Pi 5 kiosk image that boots
straight into `igrf-app`. The design is in
[`docs/buildroot-design.md`](../docs/buildroot-design.md); the platform
decision is [ADR 0002](../docs/adr/0002-buildroot-kiosk-platform.md) and the
read-only-OS split is
[ADR 0001](../docs/adr/0001-read-only-os-separate-persistent-data.md).

## Pinned release

| Thing | Pin |
| --- | --- |
| Buildroot | **2026.08** (stable, released 2026-09-04) |
| Buildroot commit | `d5180309b1b66ef3b8eaccca70ad69be8e0729a1` |
| Rust (via `rust-bin`) | **1.97.1** |
| Kernel tarball | `raspberrypi/linux` `21b410140c47ffab5668399f6f143c7d7b935c8b`, `bcm2712` defconfig |
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
  igrf_raspberrypi5_defconfig     the board defconfig
board/igrf/raspberrypi5/
  genimage.cfg                    boot + rootfs + data partitions
  post-build.sh, post-image.sh    image assembly hooks
  config_5.txt, cmdline_5.txt     Raspberry Pi firmware config
  rootfs-overlay/                 files baked into the root filesystem
package/                          igrf-app, onboard and its missing deps
```

## Not verified

This tree has **not been built or booted**. It is written against the
Buildroot 2026.08 sources and the upstream package metadata, but no cross-build
has been run and there is no Raspberry Pi 5 available. Do not treat any part of
it as proven until the checks in
[`docs/buildroot-test-plan.md`](../docs/buildroot-test-plan.md) pass on
hardware.
