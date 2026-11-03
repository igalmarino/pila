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
    echo "         Start the container from inside your desktop session (pila or GNOME) so" >&2
    echo "         XDG_RUNTIME_DIR and WAYLAND_DISPLAY are set on the host." >&2
fi

if [ -d /run/host-pila ]; then
    echo "OK: pila session log mounted at /run/host-pila"
fi

if compgen -G "/dev/dri/renderD*" >/dev/null; then
    echo "OK: GPU render node available ($(echo /dev/dri/renderD*))"
else
    echo "WARNING: no /dev/dri/renderD* device; EGL rendering will fail." >&2
fi

if [ "${ENABLE_FIREWALL:-1}" != "0" ]; then
    sudo /usr/local/bin/init-firewall.sh

    # CDN-hosted domains (crates.io, docs.rs) rotate IPs every ~60s; keep
    # adding their current ones (see init-firewall.sh). Runs as dev and only
    # calls the root script briefly; the lock prevents duplicate loops.
    #
    # This script runs in a terminal session that the devcontainer tooling
    # closes as soon as we exit, which SIGHUPs everything still in it. Ignore
    # HUP *here*, before forking, so the loop inherits it: setting it in the
    # child would race with the hangup.
    lock=/tmp/firewall-refresh.lock
    trap '' HUP
    setsid nohup bash -c '
        exec 9>>'"$lock"'
        flock -n 9 || exit 0
        while sleep 30; do
            sudo -n /usr/local/bin/init-firewall.sh --refresh || true
        done
    ' >>/tmp/firewall-refresh.log 2>&1 </dev/null &
    # Confirm it started (it holds the lock while running).
    for i in 1 2 3 4 5 6; do
        sleep 0.5
        if ! flock -n "$lock" true; then
            echo "Firewall refresh running every 30s (log: /tmp/firewall-refresh.log)"
            break
        fi
        [ "$i" = 6 ] && echo "WARNING: firewall refresh loop did not start" >&2
    done
else
    echo "Firewall disabled (DEVCONTAINER_FIREWALL=0 on the host)."
fi
