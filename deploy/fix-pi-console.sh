#!/usr/bin/env bash
# Repair a Pi whose screen shows nothing: no dashboard and no login prompt.
#
# This happens when getty@tty1 has been masked while macdash is not running,
# so nothing owns the console at all. The Pi is still reachable over SSH —
# only the attached screen is dead.
#
# Usage: deploy/fix-pi-console.sh g10@192.168.0.148
set -euo pipefail

HOST="${1:-}"
if [[ -z "$HOST" ]]; then
    echo "Usage: $0 <user@host>"
    exit 1
fi

echo "Repairing the console on ${HOST}"

ssh -t "$HOST" "sudo bash -c '
set -e

echo \"--- before ---\"
echo -n \"macdash:     \"; systemctl is-active macdash 2>&1 || true
echo -n \"getty@tty1:  \"; systemctl is-enabled getty@tty1.service 2>&1 || true

# Give the console back to a login prompt first, so the screen is usable
# even if the dashboard cannot start.
systemctl unmask getty@tty1.service 2>/dev/null || true

if [[ -x /usr/local/bin/macdash-tui ]]; then
    systemctl daemon-reload
    systemctl restart macdash 2>/dev/null || true
    sleep 3
    if systemctl is-active --quiet macdash; then
        systemctl stop getty@tty1.service 2>/dev/null || true
        systemctl mask getty@tty1.service 2>/dev/null || true
        echo \"--- macdash is running; the dashboard owns tty1 ---\"
    else
        systemctl start getty@tty1.service 2>/dev/null || true
        echo \"--- macdash did not start; tty1 shows a login prompt ---\"
        journalctl -u macdash -n 20 --no-pager || true
    fi
else
    systemctl start getty@tty1.service 2>/dev/null || true
    echo \"--- macdash is not installed; tty1 shows a login prompt ---\"
fi

echo \"--- after ---\"
echo -n \"macdash:     \"; systemctl is-active macdash 2>&1 || true
echo -n \"getty@tty1:  \"; systemctl is-active getty@tty1.service 2>&1 || true
'"

echo
echo "Done. The screen should now show either the dashboard or a login prompt."
