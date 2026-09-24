#!/usr/bin/env bash
# Keep diagnostic output off the dashboard. This runs before the TUI owns tty1.
set -u
exec >&2
# Kernel messages remain in the journal; only emergencies reach the panel.
sysctl -q -w kernel.printk='1 4 1 3' || true
# A mains-powered display needs continuous Wi-Fi reception. Apply immediately
# without disconnecting, and persist for reconnection through NetworkManager.
for wireless in /sys/class/net/*/wireless; do
    [ -d "$wireless" ] || continue
    dev="$(basename "$(dirname "$wireless")")"
    if command -v nmcli >/dev/null; then
        uuid="$(nmcli -g GENERAL.CON-UUID device show "$dev" 2>/dev/null)"
        if [[ -n "$uuid" && "$uuid" != -- ]]; then
            nmcli connection modify uuid "$uuid" 802-11-wireless.powersave 2 || true
        fi
    fi
    if command -v iw >/dev/null; then
        iw dev "$dev" set power_save off || true
    fi
done
