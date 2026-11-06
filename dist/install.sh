#!/usr/bin/env bash
# Builds and installs for the current user: binaries in ~/.local/bin, the service, and the launcher.
# The udev rule (deck and XLR Dock access) needs root and is installed only with --udev.
set -euo pipefail
cd "$(dirname "$0")/.."

name=fluxdeck
bin="$HOME/.local/bin"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

cargo build --release
# Only a first install turns on starting with systemd; later updates keep the user's choice.
first_install=0
[[ -f "$units/$name.service" ]] || first_install=1
install -Dm755 "target/release/$name" "$bin/$name"
install -Dm755 "target/release/$name-editor" "$bin/$name-editor"
install -Dm644 "dist/$name.service" "$units/$name.service"
install -Dm644 "dist/$name-editor.desktop" "$apps/$name-editor.desktop"
install -Dm644 "assets/$name.svg" "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/$name.svg"
# The Ryoku bar widget has its own repository; when it sits next to this one, its source is
# copied here for Settings → Extensions to install when the user asks.
widget="${XDG_DATA_HOME:-$HOME/.local/share}/$name/ryoku-widget"
if [[ -d "../$name-widget" ]]; then
    rm -rf "$widget"
    mkdir -p "$widget"
    cp -r "../$name-widget/." "$widget/"
    rm -rf "$widget/.git"
fi

# Swap the running copy for the new one, however it was started.
pkill -x "$name" 2>/dev/null || true
systemctl --user daemon-reload
if [[ $first_install == 1 ]]; then
    systemctl --user enable "$name.service"
fi
if systemctl --user is-enabled --quiet "$name.service"; then
    systemctl --user restart "$name.service"
else
    setsid "$bin/$name" >/dev/null 2>&1 < /dev/null &
fi

if [[ "${1:-}" == "--udev" ]]; then
    sudo install -Dm644 "dist/70-$name.rules" "/etc/udev/rules.d/70-$name.rules"
    sudo udevadm control --reload
    sudo udevadm trigger --action=add --subsystem-match=usb --attr-match=idVendor=0fd9
    sudo udevadm trigger --action=add --subsystem-match=hidraw
fi

echo "Listo: FluxDeck está actualizado y el editor está en el menú de apps. Cómo arranca se elige en Ajustes → General."
