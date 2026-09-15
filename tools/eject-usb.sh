#!/bin/sh
set -eu

[ "$#" -eq 1 ] || { echo 'usage: eject-usb.sh sda1' >&2; exit 2; }

# Same partition-name rule as usb-media.py and the polkit rule: a partition
# number starts at 1, so `sda`, `sda0` and `sda01` are rejected up front.
printf '%s\n' "$1" | grep -Eq '^sd[a-z]+[1-9][0-9]*$' || {
    echo 'usage: eject-usb.sh sda1' >&2
    exit 2
}

# A narrow polkit rule allows igrf to start only the eject helper.
# Caller must close its log file first; a busy mount fails instead of ejecting.
systemctl start "igrf-usb-eject@$1.service"
