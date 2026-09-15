#!/bin/sh
set -eu

# Xorg opens DRM/input as root; only the client session runs as igrf.
export DISPLAY=:0
export XAUTHORITY=/run/igrf-x11/Xauthority
umask 077
xauth -f "$XAUTHORITY" add "$DISPLAY" . "$(mcookie)"
chown igrf:igrf "$XAUTHORITY"
exec xinit /bin/su -s /bin/sh -c /etc/X11/xinit/xinitrc igrf -- \
    /usr/bin/Xorg "$DISPLAY" vt7 -auth "$XAUTHORITY" \
    -nolisten tcp -noreset -dpms -s 0
