#!/usr/bin/python3
"""Mount only USB FAT/exFAT, eject only the matching managed mount."""
import ctypes
import json
import os
from pathlib import Path
import re
import subprocess
import sys


def paths(name):
    if not re.fullmatch(r"sd[a-z]+(?:[1-9][0-9]*)?", name):
        raise ValueError("Expected a USB disk/partition name such as sda1")
    return f"/dev/{name}", f"/run/media/igrf-{name}"


def mount(name):
    device, destination = paths(name)
    properties = subprocess.check_output(
        ["udevadm", "info", "--query=property", "--name", device], text=True).splitlines()
    if "ID_BUS=usb" not in properties:
        raise ValueError("Refusing non-USB device")
    kind = subprocess.check_output(
        ["blkid", "-p", "-s", "TYPE", "-o", "value", device], text=True).strip()
    if kind not in ("vfat", "exfat"):
        raise ValueError("Only FAT32 and exFAT are supported")
    subprocess.run(["mount", "-t", kind, "-o",
                    "rw,nodev,nosuid,noexec,uid=1000,gid=1000,fmask=0177,dmask=0077",
                    device, destination], check=True)


def flush(destination):
    fd = os.open(destination, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        libc = ctypes.CDLL(None, use_errno=True)
        if libc.syncfs(fd) != 0:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))
    finally:
        os.close(fd)


def eject(name):
    device, destination = paths(name)
    result = json.loads(subprocess.check_output(
        ["findmnt", "--json", "--mountpoint", destination,
         "--output", "SOURCE,TARGET,FSTYPE"], text=True))
    filesystems = result.get("filesystems", [])
    if (len(filesystems) != 1 or filesystems[0]["source"] != device
            or filesystems[0]["target"] != destination
            or filesystems[0]["fstype"] not in ("vfat", "exfat")):
        raise ValueError("Not the expected managed USB mount")
    flush(destination)
    # The kernel rejects a busy filesystem. No -l or -f fallback.
    subprocess.run(["umount", destination], check=True)
    print(f"Safe to remove {name}")


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in ("mount", "eject"):
        sys.exit("usage: usb-media.py mount|eject DEVICE")
    try:
        (mount if sys.argv[1] == "mount" else eject)(sys.argv[2])
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"USB operation failed: {error}")
