#!/usr/bin/env bash
# Removes what dist/install.sh installed for the current user: binaries, service, launcher, icon
# and the autostart entry. Settings and data stay unless --purge is given.
# The udev rule needs root and is removed only with --udev.
set -euo pipefail

name=fluxdeck
bin="$HOME/.local/bin"
config="${XDG_CONFIG_HOME:-$HOME/.config}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
state="${XDG_STATE_HOME:-$HOME/.local/state}"

udev=0
purge=0
for arg in "$@"; do
    case "$arg" in
        --udev) udev=1 ;;
        --purge) purge=1 ;;
        *) echo "Uso: dist/uninstall.sh [--udev] [--purge]" >&2; exit 1 ;;
    esac
done

systemctl --user disable --now "$name.service" 2>/dev/null || true
pkill -x "$name" 2>/dev/null || true
rm -f "$bin/$name" "$bin/$name-editor" \
    "$config/systemd/user/$name.service" \
    "$data/applications/$name-editor.desktop" \
    "$data/icons/hicolor/scalable/apps/$name.svg" \
    "$config/autostart/$name.desktop"
rm -rf "$data/$name/ryoku-widget"
systemctl --user daemon-reload 2>/dev/null || true

if [[ $udev == 1 ]]; then
    sudo rm -f "/etc/udev/rules.d/70-$name.rules"
    sudo udevadm control --reload
fi

if [[ $purge == 1 ]]; then
    rm -rf "$config/$name" "$data/$name" "$state/$name"
fi

echo "Listo: FluxDeck está desinstalado."
if [[ $purge == 0 ]]; then
    echo "Tu configuración y tus datos siguen en ~/.config/$name y ~/.local/share/$name; --purge los borra."
fi
