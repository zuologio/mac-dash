#!/usr/bin/env bash
# Run on the Pi: sudo bash preview-portrait.sh [seconds] [test-binary]
# Temporarily replace the dashboard and rotate its console. No boot settings,
# installed binaries, fonts, or persistent service configuration are changed.
set -euo pipefail

DURATION=${1:-120}
BINARY=${2:-$(dirname "$0")/macdash-portrait}
ROTATE=/sys/class/graphics/fbcon/rotate
UNIT="macdash-portrait-preview-$$"

if [[ $EUID -ne 0 ]]; then
    echo "Run with sudo on the Pi." >&2
    exit 1
fi
if [[ ! $DURATION =~ ^[0-9]{1,3}$ ]] || (( 10#$DURATION < 10 || 10#$DURATION > 600 )); then
    echo "Preview duration must be 10–600 seconds (default 120)." >&2
    exit 1
fi
DURATION=$((10#$DURATION))
if [[ ! -r $ROTATE || ! -w $ROTATE ]]; then
    echo "This kernel does not expose writable framebuffer-console rotation." >&2
    exit 1
fi
if [[ ! -x $BINARY ]]; then
    echo "Copy the new Pi binary to $BINARY first." >&2
    exit 1
fi
BINARY=$(realpath "$BINARY")
if ! systemctl is-active --quiet macdash.service; then
    echo "The installed dashboard must be running before starting a preview." >&2
    exit 1
fi
PREVIOUS_ROTATION=$(cat "$ROTATE")
if [[ ! $PREVIOUS_ROTATION =~ ^[0-3]$ ]]; then
    echo "Cannot determine the current console orientation." >&2
    exit 1
fi
PID=$(systemctl show -p MainPID --value macdash.service)
ORIGINAL_ARGS=()
while IFS= read -r -d '' arg; do
    ORIGINAL_ARGS+=("$arg")
done < "/proc/$PID/cmdline"
if (( ${#ORIGINAL_ARGS[@]} == 0 )); then
    echo "Cannot read the installed dashboard's arguments." >&2
    exit 1
fi

TIMER_WAS_ACTIVE=0
systemctl is-active --quiet macdash.timer && TIMER_WAS_ACTIVE=1
SERVICE_STOPPED=0
WAIT_PID=""
cleanup() {
    trap - EXIT
    local restored=1
    if [[ -n $WAIT_PID ]]; then
        kill "$WAIT_PID" 2>/dev/null || true
        wait "$WAIT_PID" 2>/dev/null || true
    fi
    systemctl stop "$UNIT.service" 2>/dev/null || true
    # Also recover if the test process terminated while HDMI was asleep.
    if [[ -w /sys/class/graphics/fb0/blank ]]; then
        echo 0 > /sys/class/graphics/fb0/blank || true
    fi
    echo "$PREVIOUS_ROTATION" > "$ROTATE" || restored=0
    if [[ $SERVICE_STOPPED = 1 ]]; then
        systemctl start macdash.service || restored=0
    fi
    if [[ $TIMER_WAS_ACTIVE = 1 ]]; then
        systemctl start macdash.timer || restored=0
    fi
    if [[ $restored = 1 ]]; then
        echo "Portrait preview finished; restored the previous orientation and dashboard."
    else
        echo "Restoration failed: check console rotation and systemctl status macdash." >&2
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

systemctl stop macdash.timer
SERVICE_STOPPED=1
systemctl stop macdash.service

# Physical panel clockwise => console image counterclockwise (kernel value 3).
echo 3 > "$ROTATE"
if [[ $(cat "$ROTATE") != 3 ]]; then
    echo "The kernel did not apply the requested rotation." >&2
    exit 1
fi

# Reuse the installed command's arguments, including font, HDMI sleep, direct
# Mac discovery and port. The unit is transient and collected when stopped.
systemd-run --quiet --collect --unit="$UNIT" \
    --property=Type=exec \
    --property=StandardInput=tty --property=StandardOutput=tty \
    --property=StandardError=journal --property=TTYPath=/dev/tty1 \
    --property=TTYReset=yes --property=TTYVHangup=yes \
    --setenv=TERM=linux -- "$BINARY" "${ORIGINAL_ARGS[@]:1}"

echo "Portrait preview active for $DURATION seconds. Ctrl-C restores it sooner."
echo "Rotate the physical panel clockwise so the right-hand connectors face down."
echo "Console grid (rows columns): $(stty -F /dev/tty1 size)"
echo "Metrics history starts fresh in the preview and when the original service returns."
sleep "$DURATION" &
WAIT_PID=$!
wait "$WAIT_PID"
WAIT_PID=""
