#!/usr/bin/env bash
# Copy the display binary to a Pi and install it as a boot service.
#
# Usage: deploy/install-pi.sh pi@192.168.0.148 [--backlight /sys/class/backlight/rpi_backlight]
set -euo pipefail

cd "$(dirname "$0")/.."

HOST="${1:-}"
if [[ -z "$HOST" ]]; then
    echo "Usage: $0 <user@host> [--backlight <sysfs-path>] [--zero2]"
    echo "   e.g. $0 pi@192.168.0.148"
    exit 1
fi
shift

BACKLIGHT=""
TARGET="arm-unknown-linux-musleabihf"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --backlight) BACKLIGHT="$2"; shift 2 ;;
        --zero2)     TARGET="armv7-unknown-linux-musleabihf"; shift ;;
        *) echo "unknown option: $1"; exit 1 ;;
    esac
done

BIN="target/${TARGET}/release/macdash-tui"
if [[ ! -f "$BIN" ]]; then
    echo "No binary at $BIN — run deploy/build-pi.sh first."
    exit 1
fi

ARGS="--name $(hostname -s 2>/dev/null || echo display)"
[[ -n "$BACKLIGHT" ]] && ARGS="$ARGS --backlight $BACKLIGHT"

echo "Installing to ${HOST}"

# The remote half is generated here, copied over, and then run as a file.
#
# The obvious `ssh host "sudo bash -s" <<EOF` does not work: piping the script
# into bash's stdin leaves sudo with no free stdin to read a password from and
# no TTY to prompt on, so it fails with "a terminal is required". Sending a
# file instead keeps stdin available, and -t gives sudo a terminal.
# Both files are staged in one directory under fixed names so a single scp
# can carry them: each extra scp is another password prompt when key auth is
# not set up.
STAGE="$(mktemp -d -t macdash)"
trap 'rm -rf "$STAGE"' EXIT
cp "$BIN" "$STAGE/macdash-tui"

cat > "$STAGE/macdash-install.sh" <<REMOTE
set -euo pipefail

systemctl stop macdash 2>/dev/null || true
install -m 755 /tmp/macdash-tui /usr/local/bin/macdash-tui
rm -f /tmp/macdash-tui

cat > /etc/systemd/system/macdash.service <<'UNIT'
[Unit]
Description=macdash display
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/macdash-tui ${ARGS}
Restart=always
RestartSec=2

# Render to the first virtual console so the dashboard appears on the
# attached screen with no keyboard and no login.
StandardInput=tty
StandardOutput=tty
TTYPath=/dev/tty1
TTYReset=yes
TTYVHangup=yes
Environment=TERM=linux

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable macdash
systemctl restart macdash
sleep 1
systemctl --no-pager --lines=10 status macdash || true
REMOTE

echo "Copying files…"
scp -q "$STAGE/macdash-tui" "$STAGE/macdash-install.sh" "${HOST}:/tmp/"

echo "Installing…"
# -t allocates a TTY so sudo can prompt for its password; the script runs from
# a file rather than stdin so that sudo's stdin stays free to read it.
ssh -t "$HOST" "sudo bash /tmp/macdash-install.sh; rm -f /tmp/macdash-install.sh"

echo
echo "Installed. The dashboard is on tty1 and starts at boot."
echo
echo "  logs:    ssh ${HOST} 'journalctl -u macdash -f'"
echo "  restart: ssh ${HOST} 'sudo systemctl restart macdash'"
echo
echo "To skip the password prompts next time: ssh-copy-id ${HOST}"
