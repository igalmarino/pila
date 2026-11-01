#!/usr/bin/env bash
# Runs on every container start (postStartCommand).
set -euo pipefail

# Private Wayland runtime dir for the nested compositor's own socket.
runtime_dir="${XDG_RUNTIME_DIR:-/tmp/xdg-runtime}"
mkdir -p "$runtime_dir"
chmod 0700 "$runtime_dir"

# Sanity checks for nested compositor development.
if [ -S /run/host-wayland/wayland-0 ]; then
    echo "OK: host Wayland socket mounted"
else
    echo "WARNING: host Wayland socket not found at /run/host-wayland/wayland-0." >&2
    echo "         Start the container from inside your GNOME session so" >&2
    echo "         XDG_RUNTIME_DIR and WAYLAND_DISPLAY are set on the host." >&2
fi

if compgen -G "/dev/dri/renderD*" >/dev/null; then
    echo "OK: GPU render node available ($(echo /dev/dri/renderD*))"
else
    echo "WARNING: no /dev/dri/renderD* device; EGL rendering will fail." >&2
fi

if [ "${ENABLE_FIREWALL:-1}" != "0" ]; then
    sudo /usr/local/bin/init-firewall.sh
else
    echo "Firewall disabled (DEVCONTAINER_FIREWALL=0 on the host)."
fi
