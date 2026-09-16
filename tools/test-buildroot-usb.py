#!/usr/bin/env python3
"""Exercise eject failure propagation without mounting or touching devices."""
import importlib.util
import json
from pathlib import Path
import subprocess
from unittest.mock import patch

path = Path(__file__).resolve().parents[1] / "buildroot/board/igrf/raspberrypi5/rootfs-overlay/usr/lib/igrf/usb-media.py"
spec = importlib.util.spec_from_file_location("usb", path)
usb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(usb)

for name in ("../sda1", "mmcblk0p3", "sda1;reboot", "sda0", "sda1/child"):
    try:
        usb.paths(name)
    except ValueError:
        pass
    else:
        raise AssertionError(f"unsafe name accepted: {name}")

mount = json.dumps({"filesystems": [{"source": "/dev/sda1",
    "target": "/run/media/igrf-sda1", "fstype": "exfat"}]})
with patch.object(usb.subprocess, "check_output", return_value=mount):
    with patch.object(usb, "flush", side_effect=OSError("write failed")), \
         patch.object(usb.subprocess, "run") as unmount:
        try:
            usb.eject("sda1")
        except OSError:
            pass
        else:
            raise AssertionError("flush failure must fail eject")
        unmount.assert_not_called()
    with patch.object(usb, "flush") as flush, \
         patch.object(usb.subprocess, "run", side_effect=subprocess.CalledProcessError(32, "umount")):
        try:
            usb.eject("sda1")
        except subprocess.CalledProcessError:
            pass
        else:
            raise AssertionError("busy unmount must fail eject")
        flush.assert_called_once()
    with patch.object(usb, "flush"), patch.object(usb.subprocess, "run") as unmount:
        usb.eject("sda1")
        unmount.assert_called_once_with(["umount", "/run/media/igrf-sda1"], check=True)

print("USB checks passed: invalid paths, flush errors and busy unmounts rejected.")
