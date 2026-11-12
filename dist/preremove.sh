#!/bin/sh
# Before removing the package (not on an upgrade): stop the running service. It is a user
# service, so root cannot reach it with systemctl; the stop signal is a clean exit for systemd.
case "$1" in
    remove|0) pkill -x fluxdeck 2>/dev/null || true ;;
esac
exit 0
