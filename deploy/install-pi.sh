#!/usr/bin/env bash
# Copy the display binary to a Pi and install it as a boot service.
#
# Usage: deploy/install-pi.sh pi@192.168.0.148 [--backlight /sys/class/backlight/rpi_backlight]
set -euo pipefail

cd "$(dirname "$0")/.."

HOST="${1:-}"
if [[ -z "$HOST" ]]; then
    echo "Usage: $0 <user@host> [--backlight <sysfs-path>] [--mac <ip>]"
    echo "          [--font <console-font>] [--zero2]"
    echo
    echo "  --ttf    a .ttf/.otf to rasterize into a console font, for a"
    echo "           look the distro does not ship (DepartureMono,"
    echo "           BigBlueTerminal). Needs freetype-py on this machine."
    echo "  --ttf-size  pixel size for --ttf (default 24)."
    echo "  --font   console font to patch and load, without the .psf.gz."
    echo "           Default Lat15-Terminus32x16 (16x32 cells: 120x33 on a"
    echo "           1080p panel, big enough to read from a desk away)."
    echo "           Lat15-VGA16 gives the authentic IBM look at 240x67."
    echo "   e.g. $0 pi@192.168.0.148"
    exit 1
fi
shift

BACKLIGHT=""
MAC_ADDR=""
# Cell size decides the character grid, and the grid is what the layout is
# tuned for: a 1080p panel at 16x32 is 120x33, close to the 128x37 the widgets
# assume, while the same panel at 8x16 is 240x67 and leaves the dashboard
# rattling around in a grid twice the size it was designed for.
CONSOLE_FONT="Lat15-Terminus32x16"
# A .ttf/.otf to rasterize instead, for a font the distro does not ship.
TTF_FONT=""
TTF_SIZE="24"
TARGET="arm-unknown-linux-musleabihf"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --backlight) BACKLIGHT="$2"; shift 2 ;;
        --font)      CONSOLE_FONT="$2"; shift 2 ;;
        --ttf)       TTF_FONT="$2"; shift 2 ;;
        --ttf-size)  TTF_SIZE="$2"; shift 2 ;;
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
cp "$(dirname "$0")/make-console-font.py" "$STAGE/macdash-mkfont.py"

# A .ttf has to be rasterized to a bitmap before the console can use it, and
# that needs freetype -- which is on this machine, not necessarily on a Pi
# Zero. So it happens here and only the finished .psf crosses the network.
if [[ -n "$TTF_FONT" ]]; then
    if [[ ! -f "$TTF_FONT" ]]; then
        echo "No font at $TTF_FONT"
        exit 1
    fi
    echo "Rasterizing $(basename "$TTF_FONT") at ${TTF_SIZE}px…"
    if ! python3 "$(dirname "$0")/ttf-to-console-font.py" \
            "$TTF_FONT" "$TTF_SIZE" "$STAGE/macdash-font.psf"; then
        echo "Could not rasterize it; install freetype-py or drop --ttf."
        exit 1
    fi
fi

cat > "$STAGE/macdash-install.sh" <<REMOTE
set -euo pipefail

systemctl stop macdash 2>/dev/null || true
install -m 755 /tmp/macdash-tui /usr/local/bin/macdash-tui
rm -f /tmp/macdash-tui

# The console cannot draw this dashboard with any font Raspberry Pi OS ships.
# Every one of them is missing the partial-block characters the sparklines and
# meters are built from (U+2581-2587, U+2589-258F, U+25B0/25B1), and the
# console is capped at 512 glyphs per font, so there is no fuller font to
# install. The kernel substitutes a fallback glyph, which is why an unpatched
# panel shows a row of '#' where the bars should be.
#
# So the glyphs are synthesized -- they are pure geometry -- and spliced into
# a stock font over codepoints this display will never show.
# A font rasterized on the build machine wins: it is what was asked for, and
# the Pi may have no freetype to make one itself.
if [ -f /tmp/macdash-font.psf ]; then
    mkdir -p /usr/local/share/consolefonts
    install -m 644 /tmp/macdash-font.psf /usr/local/share/consolefonts/macdash.psf
    rm -f /tmp/macdash-font.psf /tmp/macdash-mkfont.py
    echo "  console font: rasterized from the supplied .ttf"
else
FONT_SRC=""
for candidate in ${CONSOLE_FONT} Lat15-Terminus32x16 Lat15-Terminus16 Lat15-VGA16; do
    if [ -f "/usr/share/consolefonts/\${candidate}.psf.gz" ]; then
        FONT_SRC="/usr/share/consolefonts/\${candidate}.psf.gz"
        break
    fi
done
if [ -n "\$FONT_SRC" ] && command -v python3 >/dev/null; then
    mkdir -p /usr/local/share/consolefonts
    if python3 /tmp/macdash-mkfont.py "\$FONT_SRC" \
            /usr/local/share/consolefonts/macdash.psf; then
        echo "  console font built from \$(basename "\$FONT_SRC")"
    else
        echo "  WARNING: could not build the console font; bars will show as '#'"
    fi
fi
rm -f /tmp/macdash-mkfont.py
fi

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
# The kernel logs to tty1 too (console=tty1 on the Pi's cmdline), and it does
# so whenever it likes -- WiFi power-save messages land minutes after boot, on
# top of the UI. A one-shot clear cannot fix that, so raise the console log
# level first: only genuine emergencies reach the panel. journalctl still
# records everything.
ExecStartPre=-/sbin/sysctl -q -w kernel.printk="1 4 1 3"
# Load the patched font, without which every bar renders as '#'.
ExecStartPre=-/bin/sh -c '[ -f /usr/local/share/consolefonts/macdash.psf ] && setfont /usr/local/share/consolefonts/macdash.psf -C /dev/tty1'
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
SCP_FILES=("$STAGE/macdash-tui" "$STAGE/macdash-install.sh" "$STAGE/macdash-mkfont.py")
[[ -f "$STAGE/macdash-font.psf" ]] && SCP_FILES+=("$STAGE/macdash-font.psf")
scp -q "${SCP_FILES[@]}" "${HOST}:/tmp/"

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
