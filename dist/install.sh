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
install -Dm755 "target/release/$name" "$bin/$name"
install -Dm755 "target/release/$name-editor" "$bin/$name-editor"
install -Dm644 "dist/$name.service" "$units/$name.service"
install -Dm644 "dist/$name-editor.desktop" "$apps/$name-editor.desktop"
install -Dm644 "assets/$name.svg" "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/$name.svg"

# A copy started by hand would fight the service for the deck.
pkill -x "$name" 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable --now "$name.service"

if [[ "${1:-}" == "--udev" ]]; then
    sudo install -Dm644 "dist/70-$name.rules" "/etc/udev/rules.d/70-$name.rules"
    sudo udevadm control --reload
    sudo udevadm trigger --action=add --subsystem-match=hidraw --subsystem-match=usb
fi

echo "Listo: el servicio arranca solo con tu sesión y el editor está en el menú de apps."
