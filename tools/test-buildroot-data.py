#!/usr/bin/env python3
"""Bounded host checks on temporary regular files, never real block devices."""
import importlib.util
import os
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "buildroot/board/igrf/raspberrypi5/rootfs-overlay/usr/lib/igrf"


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


grow = load("grow-data")
seed = load("seed-data")
with tempfile.TemporaryDirectory(prefix="igrf-data-test-") as directory:
    work = Path(directory)
    disk = work / "card.img"
    with disk.open("wb") as output:
        output.truncate(64 * 1024 * 1024)
    subprocess.run(["sfdisk", str(disk)], text=True, check=True,
                   stdout=subprocess.DEVNULL, input="""label: dos
label-id: 0x49475246
unit: sectors

start=2048, size=8192, type=c, bootable
start=10240, size=8192, type=83
start=18432, size=16384, type=83
""")
    before = grow.read_table(disk)["partitions"]
    with disk.open("r+b") as output:
        output.seek(before[2]["start"] * 512)
        output.write(b"existing filesystem data")
    grow.grow_partition(disk, disk.stat().st_size // 512)
    after = grow.read_table(disk)["partitions"]
    assert after[:2] == before[:2]
    assert after[2]["start"] == before[2]["start"]
    assert after[2]["start"] + after[2]["size"] == 64 * 1024 * 2
    with disk.open("rb") as source:
        source.seek(before[2]["start"] * 512)
        assert source.read(24) == b"existing filesystem data"
    grow.grow_partition(disk, disk.stat().st_size // 512)  # repeated boot
    assert grow.read_table(disk)["partitions"] == after
    try:
        grow.grow_partition(disk, 32 * 1024 * 2)
    except RuntimeError:
        pass
    else:
        raise AssertionError("must reject shrinking")
    subprocess.run(["sfdisk", "--disk-id", str(disk), "0x12345678"], check=True,
                   stdout=subprocess.DEVNULL)
    try:
        grow.grow_partition(disk, disk.stat().st_size // 512)
    except RuntimeError:
        pass
    else:
        raise AssertionError("must reject a foreign image")

    template, config = work / "example.json", work / "SystemConfig.json"
    template.write_text('{"example": true}')
    seed.seed(template, config, os.getuid(), os.getgid())
    assert json.loads(config.read_text()) == {"example": True, "Display": {"Mode": "Fullscreen"}}
    assert config.stat().st_mode & 0o777 == 0o600
    config.write_text("damaged but must be preserved")
    seed.seed(template, config, os.getuid(), os.getgid())
    assert config.read_text() == "damaged but must be preserved"
    assert not list(work.glob(".seed-*"))

print("Data checks passed: growth, preservation, retries, rejection and atomic seeding.")
