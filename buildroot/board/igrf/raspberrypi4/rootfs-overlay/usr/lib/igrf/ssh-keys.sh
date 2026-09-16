#!/bin/sh
set -eu
umask 077
# Preserve existing keys, including damaged keys for administrator recovery.
if [ ! -e /data/ssh/ssh_host_ed25519_key ]; then
    ssh-keygen -q -t ed25519 -N '' -f /data/ssh/ssh_host_ed25519_key
fi
