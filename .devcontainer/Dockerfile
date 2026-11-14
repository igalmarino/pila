# Dev container for a Rust/Smithay Wayland compositor, with Claude Code.
#
# Arch base (rolling, so Mesa, libwayland and friends are recent); it runs on
# any Linux host with Docker.
FROM archlinux:base-devel

ARG TZ=UTC
ARG USERNAME=dev
ARG USER_UID=1000
ARG USER_GID=1000
# Set to "true" to give the dev user passwordless sudo for everything.
# Leave "false" if you plan to run Claude Code with --dangerously-skip-permissions.
ARG FULL_SUDO=false

ENV TZ="${TZ}" \
    DEVCONTAINER=true \
    LANG=C.UTF-8

# System packages:
#   - general dev tooling
#   - Rust toolchain manager + fast linker
#   - Smithay build/runtime deps (winit + future DRM/udev/libinput backends)
#   - Wayland test clients to run inside your compositor
#   - a machine id: without one D-Bus can't start a session bus, and notify-send
#     (scripts/check's Noctalia scenario) fails
#   - ffmpeg: scripts/demo-video encodes the demo video
#   - what building Noctalia from source takes beyond its package's runtime
#     dependencies (its BUILDING.md), for work on its Pila backend
RUN pacman -Syu --noconfirm --needed \
        git openssh github-cli less vim nano jq ripgrep fd unzip \
        rustup clang mold pkgconf gdb shellcheck cargo-audit \
        wayland wayland-protocols libxkbcommon libinput systemd-libs seatd \
        mesa libglvnd libdisplay-info pixman \
        foot kitty weston wayland-utils mesa-utils ttf-dejavu gtk4-demos \
        fuzzel waybar swaybg mako libnotify swaylock wl-clipboard wlr-randr \
        xorg-xwayland xterm xorg-xeyes xorg-xdpyinfo \
        grim slurp wtype wlopm wlsunset noctalia quickshell dux-shell ffmpeg \
        meson just wireplumber nlohmann-json stb \
    && rm -rf /var/cache/pacman/pkg/* \
    && dbus-uuidgen --ensure=/etc/machine-id

# Non-root user. On Linux, the Dev Containers tooling rewrites this UID/GID to
# match your host user (updateRemoteUserUID), which is what lets the container
# connect to your host's Wayland socket.
# The password (same as the user name) is a throwaway so screen lockers
# (swaylock, via PAM) can be unlocked in nested tests. It grants nothing else:
# there's no sudo (unless FULL_SUDO), and the user isn't in wheel.
RUN groupadd -g "${USER_GID}" "${USERNAME}" \
    && useradd -m -u "${USER_UID}" -g "${USER_GID}" -s /bin/bash "${USERNAME}" \
    && echo "${USERNAME}:${USERNAME}" | chpasswd \
    && mkdir -p /workspace /commandhistory \
        "/home/${USERNAME}/.claude" \
        "/home/${USERNAME}/.config/gh" \
        "/home/${USERNAME}/.cargo/registry" \
        "/home/${USERNAME}/.local/bin" \
    && touch /commandhistory/.bash_history \
    && chown -R "${USERNAME}:${USERNAME}" /workspace /commandhistory "/home/${USERNAME}"

# Startup script.
COPY post-start.sh /usr/local/bin/
RUN chmod 0755 /usr/local/bin/post-start.sh \
    && if [ "${FULL_SUDO}" = "true" ]; then \
         echo "${USERNAME} ALL=(ALL) NOPASSWD: ALL" > "/etc/sudoers.d/${USERNAME}" \
         && chmod 0440 "/etc/sudoers.d/${USERNAME}"; \
       fi

USER ${USERNAME}
WORKDIR /workspace
ENV PATH="/home/${USERNAME}/.local/bin:/home/${USERNAME}/.cargo/bin:${PATH}" \
    EDITOR=vim \
    VISUAL=vim

# Rust stable + editor/lint components; link with mold for faster rebuilds.
RUN rustup default stable \
    && rustup component add rust-analyzer clippy rustfmt rust-src \
    && printf '%s\n' \
        '[target.x86_64-unknown-linux-gnu]' \
        'linker = "clang"' \
        'rustflags = ["-C", "link-arg=-fuse-ld=mold"]' \
        > "/home/${USERNAME}/.cargo/config.toml"

# Persistent shell history (the /commandhistory volume survives rebuilds).
RUN printf '%s\n' \
        "export PROMPT_COMMAND='history -a'" \
        'export HISTFILE=/commandhistory/.bash_history' \
        >> "/home/${USERNAME}/.bashrc"

# Claude Code, native installer (installs to ~/.local/bin and auto-updates).
RUN curl -fsSL https://claude.ai/install.sh | bash \
    && claude --version
