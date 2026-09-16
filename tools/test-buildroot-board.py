#!/usr/bin/env python3
"""Host-only hook checks: python3 tools/test-buildroot-board.py.

Does not validate genimage output or boot hardware.
"""
import os
from pathlib import Path
import subprocess
import tempfile

BOARD = Path(__file__).resolve().parents[1] / "buildroot/board/igrf/raspberrypi4"

with tempfile.TemporaryDirectory(prefix="igrf board ") as directory:
    work = Path(directory)
    target = work / "target"
    for _ in range(2):
        subprocess.run([str(BOARD / "post-build.sh"), str(target)], check=True)
        assert all((target / name).is_dir() for name in ("data", "media", "mnt"))
        (target / "data/keep").write_text("existing data")
    assert (target / "data/keep").read_text() == "existing data"

    env = dict(os.environ, HOST_DIR=str(work / "host"))
    result = subprocess.run([str(BOARD / "post-image.sh")], cwd=work,
                            env=env, capture_output=True, text=True)
    assert result.returncode != 0 and "BR2_PACKAGE_HOST_E2FSPROGS" in result.stderr

    mke2fs = work / "host/sbin/mke2fs"
    mke2fs.parent.mkdir(parents=True)
    mke2fs.write_text("#!/bin/sh\nexit 0\n")
    mke2fs.chmod(0o755)
    wrapper = work / "support/scripts/genimage.sh"
    wrapper.parent.mkdir(parents=True)
    wrapper.write_text('#!/bin/sh\n[ "$1" = "-c" ] || exit 2\n'
                       '[ "$2" = "$EXPECTED_CONFIG" ] || exit 3\nexit 17\n')
    wrapper.chmod(0o755)
    env["EXPECTED_CONFIG"] = str(BOARD / "genimage.cfg")
    result = subprocess.run([str(BOARD / "post-image.sh")], cwd=work, env=env)
    assert result.returncode == 17, "must forward config and propagate genimage failure"

print("Board hook checks passed (no image build or hardware test).")
