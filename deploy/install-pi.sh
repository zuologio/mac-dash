#!/usr/bin/env bash
# Copy the display binary to a Pi and install it as a boot service.
#
# Usage: deploy/install-pi.sh pi@192.168.0.148 [--backlight /sys/class/backlight/rpi_backlight]
set -euo pipefail

cd "$(dirname "$0")/.."

HOST="${1:-}"
if [[ -z "$HOST" ]]; then
    echo "Usage: $0 <user@host> [--backlight <sysfs-path>] [--mac <ip>] [--zero2]"
    echo "   e.g. $0 pi@192.168.0.148"
    exit 1
fi
shift

BACKLIGHT=""
MAC_ADDR=""
TARGET="arm-unknown-linux-musleabihf"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --backlight) BACKLIGHT="$2"; shift 2 ;;
        --mac)       MAC_ADDR="$2"; shift 2 ;;
        --zero2)     TARGET="armv7-unknown-linux-musleabihf"; shift ;;
        *) echo "unknown option: $1"; exit 1 ;;
    esac
done

# Many consumer access points drop multicast between wireless and wired
# clients, which makes beacon discovery impossible on an otherwise healthy
# network. Default to announcing directly to this Mac; multicast still runs
# alongside it, so a second Mac is still discovered automatically.
if [[ -z "$MAC_ADDR" ]]; then
    MAC_ADDR="$(ipconfig getifaddr en0 2>/dev/null \
                || ipconfig getifaddr en1 2>/dev/null || true)"
    [[ -n "$MAC_ADDR" ]] && echo "Announcing directly to this Mac at ${MAC_ADDR}"
fi

BIN="target/${TARGET}/release/macdash-tui"
if [[ ! -f "$BIN" ]]; then
    echo "No binary at $BIN — run deploy/build-pi.sh first."
    exit 1
fi

ARGS="--name $(hostname -s 2>/dev/null || echo display)"
[[ -n "$BACKLIGHT" ]] && ARGS="$ARGS --backlight $BACKLIGHT"
[[ -n "$MAC_ADDR" ]] && ARGS="$ARGS --mac $MAC_ADDR"

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

# The dashboard and a login prompt cannot both own tty1. Raspberry Pi OS
# ships getty@tty1 with an autologin drop-in, and leaving it running means
# the two fight over the console. Masking it is correct — but ONLY once the
# dashboard is actually installed and working, or tty1 ends up with nothing
# attached at all: a blank screen, no prompt, and no way in except SSH.
#
# So: unmask it now, and mask it again only after the service starts cleanly.
systemctl unmask getty@tty1.service 2>/dev/null || true

cat > /etc/systemd/system/macdash.service <<'UNIT'
[Unit]
Description=macdash display
# Start only once boot has settled. Starting at network.target puts the
# dashboard on tty1 while cloud-init and the rest of the boot sequence are
# still writing to that same console, so their output scrawls over the UI.
After=network-online.target cloud-init.target systemd-user-sessions.service
Wants=network-online.target

[Service]
Type=simple
# Blank the console before drawing, so late boot output does not sit under
# the UI. The TUI also repaints fully every few seconds as a backstop.
ExecStartPre=/bin/sh -c 'clear >/dev/tty1 2>/dev/null || true'
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

# Verify it actually came up before taking the console away from getty.
sleep 3
if systemctl is-active --quiet macdash; then
    # The dashboard owns tty1 now, so the login prompt must not also try.
    systemctl stop getty@tty1.service 2>/dev/null || true
    systemctl mask getty@tty1.service 2>/dev/null || true
    echo "macdash is running; tty1 handed over to the dashboard."
else
    # Leave getty enabled so the screen still shows a usable login rather
    # than nothing at all, and say what went wrong.
    systemctl unmask getty@tty1.service 2>/dev/null || true
    systemctl start getty@tty1.service 2>/dev/null || true
    echo "macdash FAILED to start — tty1 left with a login prompt." >&2
    systemctl --no-pager --lines=20 status macdash >&2 || true
    journalctl -u macdash -n 20 --no-pager >&2 || true
    exit 1
fi

systemctl --no-pager --lines=10 status macdash || true
REMOTE

echo "Copying files…"
scp -q "$STAGE/macdash-tui" "$STAGE/macdash-install.sh" "${HOST}:/tmp/"

echo "Installing…"
# -t allocates a TTY so sudo can prompt for its password; the script runs from
# a file rather than stdin so that sudo's stdin stays free to read it.
#
# The remote exit status is checked explicitly: without this the script
# announced success even when sudo had failed and nothing was installed.
if ! ssh -t "$HOST" "sudo bash /tmp/macdash-install.sh && rm -f /tmp/macdash-install.sh"; then
    echo
    echo "Install failed on the Pi." >&2
    echo "If sudo could not read a password, run this from an interactive" >&2
    echo "terminal — the prompt needs a TTY." >&2
    exit 1
fi

echo
echo "Installed. The dashboard is on tty1 and starts at boot."
echo
echo "  logs:    ssh ${HOST} 'journalctl -u macdash -f'"
echo "  restart: ssh ${HOST} 'sudo systemctl restart macdash'"
echo
echo "To skip the password prompts next time: ssh-copy-id ${HOST}"
