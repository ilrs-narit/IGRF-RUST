#!/bin/sh
set -eu

# Buildroot passes TARGET_DIR as the first argument. Only target paths
# are created; runtime state is populated after mounting /data (#21).
mkdir -p "${1:?missing target directory}/data" "$1/media" "$1/mnt"
mkdir -p "$1/etc/NetworkManager/system-connections" "$1/var/lib/NetworkManager"
mkdir -p "$1/etc/systemd/system/local-fs.target.wants"
ln -snf ../data.mount "$1/etc/systemd/system/local-fs.target.wants/data.mount"
# Enable NetworkManager here: its Buildroot recipe installs only the D-Bus
# alias, so without this link the unit never starts.
mkdir -p "$1/etc/systemd/system/multi-user.target.wants"
ln -snf /usr/lib/systemd/system/NetworkManager.service \
    "$1/etc/systemd/system/multi-user.target.wants/NetworkManager.service"
# igrf-app.service is enabled by its package's relative .wants link
# (package/igrf-app/igrf-app.mk) and igrf-x11-session.service by the symlink
# committed in the rootfs overlay. Those are the single definitions; do not
# shadow them with a second link here.
ln -snf /run/NetworkManager/resolv.conf "$1/etc/resolv.conf"
# NM owns DHCP. Waiting for connectivity must never gate the kiosk. The
# networkd masks are preventive: networkd is not selected today, and masking
# it keeps a later defconfig change from adding a second DHCP client.
ln -snf /dev/null "$1/etc/systemd/system/NetworkManager-wait-online.service"
ln -snf /dev/null "$1/etc/systemd/system/systemd-networkd.service"
ln -snf /dev/null "$1/etc/systemd/system/systemd-networkd-wait-online.service"

# Keep wrappers and their shared implementation together in the image.
BOARD_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
for tool in backup-data.sh restore-data.sh igrf-data.py eject-usb.sh; do
    install -D -m 0755 "$BOARD_DIR/../../../../tools/$tool" "$1/usr/lib/igrf/$tool"
done
