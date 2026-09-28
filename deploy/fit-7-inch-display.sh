#!/usr/bin/env bash
# Run on the Pi with sudo; keep make-console-font.py beside this script.
# Fit the full dashboard on the final 1024x600 panel without restarting it.
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "Run this script with sudo to update the console font." >&2
    exit 1
fi
if [[ $(cat /sys/class/graphics/fb0/virtual_size) != 1024,600 ]]; then
    echo "This font preset is for a 1024x600 framebuffer." >&2
    exit 1
fi

SOURCE=/usr/share/consolefonts/Lat15-Terminus16.psf.gz
DEST=/usr/local/share/consolefonts/macdash.psf
WORK=$(mktemp -d)
APPLIED=0
DONE=0
cleanup() {
    if [[ $APPLIED = 1 && $DONE = 0 ]]; then
        setfont -C /dev/tty1 "$WORK/previous.psf" || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

python3 "$(dirname "$0")/make-console-font.py" "$SOURCE" "$WORK/macdash.psf"
# Keep a persistent copy of the previous size for an easy rollback.
if [[ -f $DEST && ! -e ${DEST}.before-7-inch ]]; then
    cp -p "$DEST" "${DEST}.before-7-inch"
fi

# Save the active font and Unicode map, then load both from the new PSF.
# ratatui notices the new terminal dimensions and redraws automatically.
setfont -C /dev/tty1 -O "$WORK/previous.psf"
APPLIED=1
setfont -C /dev/tty1 "$WORK/macdash.psf"
install -m 644 "$WORK/macdash.psf" "$DEST"
DONE=1
echo "Applied 8x16 font; dashboard grid (rows columns): $(stty -F /dev/tty1 size)"
echo "The full layout is active and this font will also load at next boot."
