#!/bin/sh
set -eu

# Xorg opens DRM/input as root; only the client session runs as igrf.
# Privilege drop uses setpriv: util-linux's su selects linux-pam, which this
# image does not carry.
export DISPLAY=:0
export XAUTHORITY=/run/igrf-x11/Xauthority
umask 077
xauth -f "$XAUTHORITY" add "$DISPLAY" . "$(mcookie)"
chown igrf:igrf "$XAUTHORITY"
exec xinit /usr/bin/setpriv --reuid 1000 --regid 1000 --init-groups \
    /bin/sh -c /etc/X11/xinit/xinitrc -- \
    /usr/bin/Xorg "$DISPLAY" vt7 -auth "$XAUTHORITY" \
    -nolisten tcp -noreset -dpms -s 0
