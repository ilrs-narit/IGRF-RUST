#!/usr/bin/python3
"""Grow only partition 3 of the IGRF MBR image; never format a filesystem."""
import json
from pathlib import Path
import stat
import subprocess


def read_table(device):
    return json.loads(subprocess.check_output(
        ["sfdisk", "--json", str(device)], text=True))["partitiontable"]


def grow_partition(device, sectors):
    table = read_table(device)
    parts = table["partitions"]
    if (table["label"] != "dos" or table.get("id") != "0x49475246"
            or table.get("sectorsize") != 512 or len(parts) != 3
            or [p["type"] for p in parts] != ["c", "83", "83"]):
        raise RuntimeError("Not the expected IGRF three-partition MBR image")
    for left, right in zip(parts, parts[1:]):
        if left["start"] + left["size"] > right["start"]:
            raise RuntimeError("Overlapping or unordered partitions")
    data = parts[2]
    size = (sectors // 2048) * 2048 - data["start"]
    if size < data["size"]:
        raise RuntimeError("Refusing to shrink data partition")
    if size == data["size"]:
        return
    subprocess.run(["sfdisk", "--lock", "--no-reread", "--no-tell-kernel",
                    "--wipe", "never", "--wipe-partitions", "never", "-N", "3",
                    str(device)], input=f"size={size}\n", text=True, check=True)
    updated = read_table(device)["partitions"]
    if updated[:2] != parts[:2] or updated[2] != dict(data, size=size):
        raise RuntimeError("Partition readback differs from intended growth")


def main():
    device = Path("/dev/mmcblk0")
    partition = "/dev/mmcblk0p3"
    if not stat.S_ISBLK(device.stat().st_mode):
        raise RuntimeError("microSD block device missing")
    mounts = Path("/proc/mounts").read_text().splitlines()
    # /proc/mounts may spell the source /dev/root. Compare device numbers.
    if (Path("/").stat().st_dev != Path("/dev/mmcblk0p2").stat().st_rdev
            or not any(line.split()[1:3] == ["/", "squashfs"] for line in mounts)):
        raise RuntimeError("Root is not the expected microSD squashfs")
    mounted = subprocess.run(["findmnt", "--noheadings", "--source", partition],
                             stdout=subprocess.DEVNULL)
    if mounted.returncode != 1 or any(line.split()[1] == "/data" for line in mounts):
        raise RuntimeError("Data must be unmounted before growth")
    for field, expected in (("TYPE", "ext4"), ("LABEL", "data")):
        actual = subprocess.check_output(
            ["blkid", "-p", "-s", field, "-o", "value", partition], text=True).strip()
        if actual != expected:
            raise RuntimeError("Data filesystem identity mismatch")
    sectors = int(subprocess.check_output(["blockdev", "--getsz", str(device)]))
    grow_partition(device, sectors)
    # Retry this even if the on-disk partition was grown before power loss.
    subprocess.run(["partx", "--update", "--nr", "3", str(device)], check=True)
    result = subprocess.run(["e2fsck", "-p", partition])
    if result.returncode not in (0, 1):
        raise RuntimeError("Data filesystem requires administrator recovery")
    subprocess.run(["resize2fs", partition], check=True)


if __name__ == "__main__":
    main()
