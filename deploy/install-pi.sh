#!/usr/bin/env bash
# Copy the display binary to a Pi and install it as a boot service.
#
# Usage: deploy/install-pi.sh pi@192.168.0.148 [--backlight /sys/class/backlight/rpi_backlight]
set -euo pipefail

cd "$(dirname "$0")/.."

HOST="${1:-}"
if [[ -z "$HOST" ]]; then
    echo "Usage: $0 <user@host> [--backlight <sysfs-path>] [--mac <ip>]"
    echo "          [--font <console-font>] [--hdmi] [--portrait|--landscape|--landscape-flipped] [--zero2]"
    echo
    echo "  --hdmi   switch off HDMI while the Mac's display sleeps (VC4/KMS)."
    echo "  --portrait  panel mounted clockwise, right-hand connectors facing down."
    echo "  --landscape restore normal orientation; otherwise keep the installed setting."
    echo "  --landscape-flipped  rotate landscape 180 degrees, connectors at bottom left."
    echo "  --ttf    a .ttf/.otf to rasterize into a console font, for a"
    echo "           look the distro does not ship (DepartureMono,"
    echo "           BigBlueTerminal). Needs freetype-py on this machine."
    echo "  --ttf-size  pixel size for --ttf (default 24)."
    echo "  --font   console font to patch and load, without the .psf.gz."
    echo "           Keeps the installed typeface on updates unless specified."
    echo "           Default Lat15-Terminus16 (8x16 cells: 128x37 on the"
    echo "           final 1024x600 panel, with all dashboard panels visible)."
    echo "           Use Lat15-Terminus32x16 for larger text on a 1080p panel."
    echo "           Lat15-VGA16 gives the authentic IBM look at 240x67."
    echo "   e.g. $0 pi@192.168.0.148"
    exit 1
fi
shift

BACKLIGHT=""
HDMI=0
ROTATION=""
MAC_ADDR=""
# The final 1024x600 panel needs 8x16 cells for the full 128x37 layout.
# Existing installs retain their selected typeface and size unless overridden.
CONSOLE_FONT="Lat15-Terminus16"
KEEP_FONT=1
# A .ttf/.otf to rasterize instead, for a font the distro does not ship.
TTF_FONT=""
TTF_SIZE="24"
TARGET="arm-unknown-linux-musleabihf"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --backlight) BACKLIGHT="$2"; shift 2 ;;
        --hdmi)      HDMI=1; shift ;;
        --portrait)  ROTATION=3; shift ;;
        --landscape) ROTATION=0; shift ;;
        --landscape-flipped) ROTATION=2; shift ;;
        --font)      CONSOLE_FONT="$2"; KEEP_FONT=0; shift 2 ;;
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
# The dashboard loads the bitmap and Unicode map after it owns the tty,
# before sizing its buffers for the font's character grid.
ARGS="$ARGS --font /usr/local/share/consolefonts/macdash.psf"
[[ -n "$BACKLIGHT" ]] && ARGS="$ARGS --backlight $BACKLIGHT"
[[ "$HDMI" = 1 ]] && ARGS="$ARGS --hdmi"
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
cp deploy/make-console-font.py "$STAGE/macdash-mkfont.py"
cp deploy/macdash.service deploy/macdash.timer deploy/prepare-pi-console.sh "$STAGE/"

# A .ttf has to be rasterized to a bitmap before the console can use it, and
# that needs freetype -- which is on this machine, not necessarily on a Pi
# Zero. So it happens here and only the finished .psf crosses the network.
if [[ -n "$TTF_FONT" ]]; then
    if [[ ! -f "$TTF_FONT" ]]; then
        echo "No font at $TTF_FONT"
        exit 1
    fi
    echo "Rasterizing $(basename "$TTF_FONT") at ${TTF_SIZE}px…"
    if ! python3 deploy/ttf-to-console-font.py \
            "$TTF_FONT" "$TTF_SIZE" "$STAGE/macdash-font.psf"; then
        echo "Could not rasterize it; install freetype-py or drop --ttf."
        exit 1
    fi
fi

# NOTE: this heredoc is UNQUOTED, deliberately -- $ARGS and $CONSOLE_FONT
# have to be expanded here, on the build machine. The cost is that every
# backtick and $( ) inside it is also expanded here, including ones written
# as prose. Do not use backticks in the comments below.
cat > "$STAGE/macdash-install.sh" <<REMOTE
set -euo pipefail

# Check before stopping the working dashboard or replacing any installed file.
if [ -n "${ROTATION}" ] && [ ! -w /sys/class/graphics/fbcon/rotate ]; then
    echo "This kernel does not expose writable framebuffer-console rotation." >&2
    exit 1
fi

trap 'if ! systemctl is-active --quiet macdash; then
          systemctl stop macdash.service macdash.timer 2>/dev/null || true
          systemctl unmask getty@tty1.service 2>/dev/null || true
          systemctl start getty@tty1.service 2>/dev/null || true
          echo "Install did not finish; tty1 returned to a login prompt." >&2
      fi' EXIT

systemctl stop macdash.timer macdash.service 2>/dev/null || true
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
elif [ "${KEEP_FONT}" = 1 ] && [ -s /usr/local/share/consolefonts/macdash.psf ]; then
    echo "  preserving the installed typeface and repairing missing symbols"
    python3 /tmp/macdash-mkfont.py /usr/local/share/consolefonts/macdash.psf \
        /usr/local/share/consolefonts/macdash.psf
    rm -f /tmp/macdash-mkfont.py
else
FONT_SRC=""
for candidate in ${CONSOLE_FONT} Lat15-Terminus32x16 Lat15-Terminus16 Lat15-VGA16; do
    if [ -f "/usr/share/consolefonts/\${candidate}.psf.gz" ]; then
        FONT_SRC="/usr/share/consolefonts/\${candidate}.psf.gz"
        break
    fi
done
if [ -z "\$FONT_SRC" ] || ! command -v python3 >/dev/null; then
    echo "Cannot build the required console font: install console fonts and python3." >&2
    exit 1
fi
mkdir -p /usr/local/share/consolefonts
python3 /tmp/macdash-mkfont.py "\$FONT_SRC" \
    /usr/local/share/consolefonts/macdash.psf
echo "  console font built from \$(basename "\$FONT_SRC")"
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

install -m 755 /tmp/prepare-pi-console.sh /usr/local/bin/macdash-prepare-console
install -m 644 /tmp/macdash.service /etc/systemd/system/macdash.service
install -m 644 /tmp/macdash.timer /etc/systemd/system/macdash.timer
mkdir -p /etc/systemd/system/macdash.service.d
# Keep orientation in its own drop-in so ordinary updates preserve it.
if [ -n "${ROTATION}" ]; then
    cat > /etc/systemd/system/macdash.service.d/orientation.conf <<'ORIENTATION'
[Service]
Environment=MACDASH_CONSOLE_ROTATION=${ROTATION}
ORIENTATION
fi
cat > /etc/systemd/system/macdash.service.d/display.conf <<'UNIT'
[Service]
ExecStart=
ExecStart=/usr/local/bin/macdash-tui ${ARGS}
UNIT
if [ "${HDMI}" = 1 ]; then
    # Restore HDMI even after SIGTERM, a crash, or a failed service start.
    printf '%s\n' 'ExecStopPost=/bin/sh -c "echo 0 > /sys/class/graphics/fb0/blank"' \
        >> /etc/systemd/system/macdash.service.d/display.conf
fi
rm -f /tmp/macdash.service /tmp/macdash.timer /tmp/prepare-pi-console.sh

# A timer starts the display after the boot jobs settle, without making
# multi-user.target wait for a unit ordered after cloud-final.service.
systemctl disable macdash.service 2>/dev/null || true

systemctl daemon-reload
systemctl enable macdash.timer

systemctl restart macdash

# Verify it actually came up before taking the console away from getty.
#
# Poll rather than sleeping a fixed three seconds: a unit that is still
# 'activating' is not a unit that failed, and treating it as one hands tty1
# back to getty, which then fights the dashboard for the console forever.
tries=0
while [ \$tries -lt 15 ]; do
    systemctl is-active --quiet macdash && break
    systemctl is-failed --quiet macdash && break
    tries=\$((tries + 1))
    sleep 1
done
sleep 2
if systemctl is-active --quiet macdash; then
    # The dashboard owns tty1 now, so the login prompt must not also try.
    systemctl stop getty@tty1.service 2>/dev/null || true
    systemctl mask getty@tty1.service 2>/dev/null || true
    systemctl start macdash.timer
    echo "macdash is running; tty1 handed over to the dashboard."
else
    # Leave getty enabled so the screen still shows a usable login rather
    # than nothing at all, and say what went wrong.
    systemctl stop macdash.service macdash.timer 2>/dev/null || true
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
# The remote script is assembled by an unquoted heredoc, so the local shell
# expands $(...) and backticks inside it -- including ones meant as prose in
# a comment. A stray backtick corrupts the script before it is even written,
# so this checks the result rather than trusting it. (The heredoc itself is
# guarded by a test in this repo's deploy checks; see the comment above it.)
if ! sh -n "$STAGE/macdash-install.sh"; then
    echo "Generated install script is malformed; refusing to send it." >&2
    exit 1
fi

SCP_FILES=("$STAGE/macdash-tui" "$STAGE/macdash-install.sh" "$STAGE/macdash-mkfont.py"
           "$STAGE/macdash.service" "$STAGE/macdash.timer" "$STAGE/prepare-pi-console.sh")
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
