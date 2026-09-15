#!/usr/bin/python3
"""Install an initial configuration atomically, preserving all existing data."""
import os
import json
from pathlib import Path
import pwd
import tempfile


def seed(source, destination, uid, gid):
    destination = Path(destination)
    if os.path.lexists(destination.parent.parent / ".igrf-restore-in-progress"):
        raise RuntimeError("Interrupted restore requires administrator recovery")
    fd, temporary = tempfile.mkstemp(prefix=".seed-", dir=destination.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            config = json.loads(Path(source).read_text())
            config.setdefault("Display", {})["Mode"] = "Fullscreen"
            output.write((json.dumps(config, indent=2) + "\n").encode())
            os.fchown(output.fileno(), uid, gid)
            output.flush()
            os.fsync(output.fileno())
        try:
            os.link(temporary, destination)  # Never overwrite, even a corrupt config.
        except FileExistsError:
            pass
    finally:
        os.unlink(temporary)
    directory = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


if __name__ == "__main__":
    user = pwd.getpwnam("igrf")
    seed("/usr/share/igrf/SystemConfig.example.json",
         "/data/igrf/SystemConfig.json", user.pw_uid, user.pw_gid)
