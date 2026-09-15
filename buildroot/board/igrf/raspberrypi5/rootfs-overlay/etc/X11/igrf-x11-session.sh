#!/bin/sh
# IGRF kiosk X11 session launcher, run as root by igrf-x11-session.service.
#
# Split of privileges:
#   - Xorg runs as root. There is no display manager and no logind session
#     on this image (PAM is not installed), so a non-root Xorg would have no
#     path to the DRM/input devices or a virtual console.
#   - The session itself (session D-Bus, matchbox, XDG autostart) runs as
#     the unprivileged kiosk user "igrf" via the xinitrc below. The session
#     bus MUST belong to igrf: D-Bus authenticates local clients by uid, and
#     igrf-app.service (User=igrf) drives Onboard over `gdbus call --session`.
#
# The app itself is deliberately NOT started here: igrf-app.service is
# ordered After=igrf-x11-session.service and starts (and restarts) the app.
#
# Everything here is static configuration baked into the read-only rootfs;
# nothing under /etc is written at runtime.

set -u

export DISPLAY=:0

# vt7: the classic X console, kept clear of the gettys on tty1+.
# -noreset/-dpms/-s 0: a kiosk panel must never blank or reset.
Xorg "$DISPLAY" vt7 -nolisten tcp -noreset -dpms -s 0 &
x_pid=$?

# Wait until the server is accepting local connections before handing the
# display to the session. Fail loudly (and let systemd restart us) if Xorg
# never comes up.
i=0
while [ ! -S /tmp/.X11-unix/X0 ]; do
    i=$((i + 1))
    if [ "$i" -gt 150 ]; then
        echo "igrf-x11-session: Xorg did not create /tmp/.X11-unix/X0" >&2
        kill "$x_pid" 2>/dev/null
        exit 1
    fi
    sleep 0.2
done

# Run the session as igrf. -s forces a real shell so this works even if the
# kiosk account's passwd shell is nologin. When the session ends (matchbox
# exits), bring X down with it; systemd restarts the whole unit.
su -s /bin/sh -c /etc/X11/xinit/xinitrc igrf
status=$?

kill "$x_pid" 2>/dev/null
wait "$x_pid" 2>/dev/null
exit "$status"
