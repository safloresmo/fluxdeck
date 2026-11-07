#!/bin/sh
# After installing the package: apply the udev rule to a deck that is already plugged in.
udevadm control --reload 2>/dev/null || true
udevadm trigger --action=add --subsystem-match=hidraw 2>/dev/null || true
udevadm trigger --action=add --subsystem-match=usb --attr-match=idVendor=0fd9 2>/dev/null || true
exit 0
