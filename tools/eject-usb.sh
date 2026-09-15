#!/bin/sh
set -eu
case "${1-}" in
    ''|*[!a-z0-9]*) echo 'usage: eject-usb.sh sda1' >&2; exit 2 ;;
esac
[ "$#" -eq 1 ] || exit 2
# A narrow polkit rule allows igrf to start only the eject helper.
# Caller must close its log file first; a busy mount fails instead of ejecting.
systemctl start "igrf-usb-eject@$1.service"
