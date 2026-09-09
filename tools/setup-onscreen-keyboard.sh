#!/usr/bin/env bash
# Set up an on-screen keyboard for the IGRF app on a touchscreen Linux box
# (e.g. a LattePanda on a 1024x600 touch panel).
#
# IMPORTANT: the app uses egui, which does NOT tell the OS when a text field is
# focused. So the keyboard will not pop up on its own when you tap a field.
# Two workable arrangements:
#
#   X11     - `onboard`, docked at the bottom of the screen, always visible.
#             `onboard-settings` also has an "auto-show on text focus" toggle,
#             but that only fires for GTK apps, not for egui - so expect to tap
#             the docked keyboard yourself.
#   Wayland - `wvkbd`, a small keyboard bar. Show it with a hotkey or a script.
#
# This script installs the right one for the current session and adds an
# autostart entry so it comes up with the app. Run it once on the device.

set -euo pipefail

session_type="${XDG_SESSION_TYPE:-${WAYLAND_DISPLAY:+wayland}}"

case "$session_type" in
  wayland)
    echo "Session: Wayland -> installing wvkbd (keyboard bar)."
    sudo apt-get update
    sudo apt-get install -y wvkbd
    # Show it with the hotkey or from a terminal:
    #   wvkbd-mobintl &
    # To make it available on a hotkey, bind e.g. Super+Space to `wvkbd-mobintl`.
    ;;
  *)
    echo "Session: X11 -> installing onboard (docked on-screen keyboard)."
    sudo apt-get update
    sudo apt-get install -y onboard
    # Dock onboard at the bottom so it is always available. Best-effort: these
    # dconf keys ship with onboard; if they do not exist on this version the
    # user can set the same thing in onboard-settings.
    if command -v gsettings >/dev/null 2>&1; then
      gsettings set org.onboard.dock expanded true || true
      gsettings set org.onboard.dock position bottom || true
      gsettings set org.onboard.dock width 100 || true
      gsettings set org.onboard.show-on-desktop false || true
      gsettings set org.onboard.auto-show enable true || true
    fi
    # Autostart a docked onboard. `--size 800x300` keeps it small on a 1024x600
    # panel; adjust to taste.
    autostart_dir="${HOME}/.config/autostart"
    mkdir -p "$autostart_dir"
    cat > "$autostart_dir/onboard.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Onboard (on-screen keyboard)
Exec=onboard --size 800x300
X-GNOME-Autostart-enabled=true
EOF
    echo "Autostart entry written to $autostart_dir/onboard.desktop"
    ;;
esac

cat <<'EOF'

Setup complete.

How it behaves with the IGRF app:
  * The app's operating screens (Control tab) use draggable number fields and
    need no keyboard at all.
  * Text fields (serial ports, IPs, file paths, TLE lines) live in Settings /
    Model tabs and are normally set once. Edit SystemConfig.json directly, or
    bring up the on-screen keyboard to type them.

To show the keyboard:
  * X11 / onboard: it is docked at the bottom. Click it, then tap a text field.
  * Wayland / wvkbd: run `wvkbd-mobintl &`, or bind it to a hotkey.

To auto-show on focus for GTK apps only (not egui): onboard-settings ->
"Show when text is focused". For egui this will not fire; keep onboard docked.
EOF
