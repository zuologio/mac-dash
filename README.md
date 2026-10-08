<div align="center">

# macdash

**Your Mac's vitals, live on a second screen.**

A tiny metrics agent for macOS and a dashboard for a Raspberry Pi, talking
over your LAN. Plug a small display into the Pi and leave it running.

On the Pi, the dashboard is a macOS-style picture drawn in pixels. Over SSH, on
the Mac, or with `--text`, you get the text dashboard shown below.

[![Rust](https://img.shields.io/badge/rust-1.75%2B-b7410e.svg)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/mac-Apple%20Silicon-black.svg)](#)
[![Pi](https://img.shields.io/badge/pi-Zero%20W%20(ARMv6)-c51a4a.svg)](#)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

</div>

```
 ● macAbre.local  up 1d 16h                                                                               08:12:56   Fri 18 Sep
┌ CPU ───────────────────────────────────────────────────────────────────┐┌ MEMORY ────────────────────────────────────────────┐
│  6.6%  usr 3.0%  sys 3.5%   load 3.66 4.21 3.87   10 cores             ││12.2G / 16.0G                                       │
│████▊···································································││███████████████████████████████████████▉············│
│cores ▂▂▁▁▁▁▂▂▁▁                                                        ││pressure 58%  compressed 6.3G  swap 11.9G / 13.0G   │
│                                                                        ││                                                    │
│                                                                        ││██████████████████████████                          │
│                                                                        │└────────────────────────────────────────────────────┘
│                                                                        │┌ THERMAL ───────────────────────────────────────────┐
│                                                                        ││ 41.4°C  SoC                                        │
│                                                                        ││ 32.0°C  batt                                       │
│                                                                        │└────────────────────────────────────────────────────┘
│                                                                        │┌ NETWORK ───────────────────────────────────────────┐
│█       ▁                                                               ││▼ 5.0K/s   ▲ 5.0K/s                                 │
│█     ▆ █            ▆▆                                                 ││█▂▂ ▆▆▄   ▂▂     ▄▄▂▂▄▄▂▂▂                          │
│█▄▄ ▅▇█ █▁   ▁   ▇▇▁▁██▅▅▁                                              ││█▂  ▂      ▂ ▂▂  ▂▂▂▂▄▄▂▂▂                          │
│███▄███▃██▄▇▆█▄▄▄█████████                                              ││session  ▼ 379M   ▲ 2.9G                            │
└────────────────────────────────────────────────────────────────────────┘└────────────────────────────────────────────────────┘
┌ TOP PROCESSES ───────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│Docker Desktop Helper (R    25.3%  92.8M  ▰▰▰▰▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│claude                      14.9%   261M  ▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│Code                         4.2%   107M  ▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
 LIVE en0 26 pkts  q quit
```

**Shows:** CPU (total, per core, user/system, load) · memory (used, pressure,
compressed, swap) · SoC and battery temperature · network throughput · disk
usage (the Mac's data volume, used and free) · top apps,
each app's helper processes counted together (3 on the pixel panel, 8 in the
text view) · the Mac's clock.

**Costs:** about 0.012% of one Mac core, 5 MB of RAM and 37 kbps on the network.
No sudo needed. The Pi binary is a single 700 KB static file.

---

## Quick start

### 1. Try it without installing

Both machines need to be on the same LAN. Run each half in a terminal:

```bash
cargo run --release -p macdash-agent      # on the Mac
cargo run --release -p macdash-tui        # on any machine on the LAN, even the Mac itself
```

The two halves find each other on their own, with no IP addresses to configure.
Press `q` to quit.

### 2. Install the agent on the Mac

```bash
./deploy/install-mac.sh
```

This builds the agent and installs it as a launchd agent, so it starts at login
and keeps running in the background.

### 3. Install the dashboard on the Pi

Building for the Pi needs Docker on the Mac and the `cross` tool:

```bash
cargo install cross --git https://github.com/cross-rs/cross
./deploy/build-pi.sh                       # add --zero2 for a Pi Zero 2 W
./deploy/install-pi.sh pi@192.168.0.148
```

The dashboard now starts on the Pi's screen at boot, with no keyboard needed,
and finds the Mac by itself. Run the same two commands again to update it.

## Install options

Pass any of these to `install-pi.sh`. They are saved in the service, so pass
them again on later installs to keep them.

| Option | What it does |
|---|---|
| `--hdmi` | Turn the HDMI screen off while the Mac's display sleeps |
| `--backlight <path>` | Same, for panels with a Linux backlight device, e.g. `/sys/class/backlight/rpi_backlight` |
| `--theme <name>` | Colour scheme (see [Themes](#themes)) |
| `--portrait` | Panel turned 90° clockwise, connectors at the bottom |
| `--landscape-flipped` | Panel upside down, connectors at the bottom left |
| `--landscape` | Back to normal orientation |
| `--font <name>` | Console font. The default `Lat15-Terminus16` fits a 1024×600 panel; use `Lat15-Terminus32x16` for larger text on a 1080p screen |
| `--ttf <file>` `--ttf-size <px>` `--ttf-cell <WxH>` | Turn your own `.ttf`/`.otf` into the console font (needs `freetype-py`). `--ttf-cell 8x16` keeps the full 128x37 layout |
| `--mac <ip>` | Also announce directly to this Mac. Use it if the screen stays on `WAITING` (some Wi-Fi routers block multicast). The installer fills this in automatically |
| `--text` | Use the text console instead of the pixel dashboard |
| `--zero2` | Install the Pi Zero 2 W build |

A typical setup for a 7-inch HDMI panel:

```bash
./deploy/install-pi.sh pi@192.168.0.148 --hdmi --theme quiet
```

Orientation and font choices are kept on later installs unless you pass a new one.

## Themes

Themes apply to the text dashboard. The Pi's pixel dashboard has one macOS-style look.

| Theme | Normal | Warning | Critical |
|---|---|---|---|
| `classic` (default) | blue (`#5293E1`) | yellow | red |
| `traffic` | green | amber | red |
| `quiet` | plain white, so colour only appears when something is wrong | amber | red |
| `thermal` | teal | warm amber | red-orange |
| `phosphor` | dark amber | bright amber | red-orange |
| `sapphire` | blue (`#5293E1`) | amber | soft red, with violet upload |
| `grey` | light grey | white | red, the only colour on screen |
| `graphite` | green | orange | red: macOS dark mode, with a window-grey background and rounded panels |

Press **`t`** while the dashboard is running to cycle through them, then set your
favourite with `--theme`. On the Pi's console, all themes except `classic`
and `grey` show their exact colours; `classic` keeps the console's own yellow
and red, and `grey` its own greys and red.

To try one locally:

```bash
cargo run --release -p macdash-tui                     # classic
cargo run --release -p macdash-tui -- --theme traffic
cargo run --release -p macdash-tui -- --theme quiet
cargo run --release -p macdash-tui -- --theme thermal
cargo run --release -p macdash-tui -- --theme phosphor
cargo run --release -p macdash-tui -- --theme sapphire
cargo run --release -p macdash-tui -- --theme grey
cargo run --release -p macdash-tui -- --theme graphite
```

To make one the Pi's default, install with it (add your other options, such
as `--hdmi`, since they are not kept between installs):

```bash
./deploy/install-pi.sh pi@192.168.0.148                     # classic
./deploy/install-pi.sh pi@192.168.0.148 --theme traffic
./deploy/install-pi.sh pi@192.168.0.148 --theme quiet
./deploy/install-pi.sh pi@192.168.0.148 --theme thermal
./deploy/install-pi.sh pi@192.168.0.148 --theme phosphor
./deploy/install-pi.sh pi@192.168.0.148 --theme sapphire
./deploy/install-pi.sh pi@192.168.0.148 --theme grey
./deploy/install-pi.sh pi@192.168.0.148 --theme graphite
```

`graphite` looks closest to macOS with JetBrains Mono Bold as the console font:

```bash
./deploy/install-pi.sh pi@192.168.0.148 --theme graphite \
    --ttf ~/Library/Fonts/JetBrainsMonoNLNerdFontMono-Bold.ttf --ttf-size 13 --ttf-cell 8x16
```

## Using it

**Pixel or text:** the pixel dashboard runs when the terminal is the Pi's own
console (`TERM=linux`, as in the service) and `/dev/fb0` works. If the
framebuffer cannot be used, it falls back to text by itself. SSH sessions and
Mac terminals always get the text dashboard. The pixel dashboard redraws twice
a second while the Mac streams, and once a second when idle.

**The Pi's own load:** right of the status pill, the pixel dashboard shows
the Pi's temperature and CPU use (`Pi 48° 12%`), updated every 5 seconds.

**Keys:** `t` cycles themes · `q`, `Esc` or `Ctrl-C` quits.

**Status line** at the bottom left:

| Status | Meaning | Screen |
|---|---|---|
| `LIVE` | data is arriving | normal |
| `DISPLAY OFF` | the Mac's display is asleep | screen off (with `--hdmi`/`--backlight`), values current |
| `STALE` | no data for 1–10 s | values dimmed |
| `ASLEEP` | no data for over 10 s | screen off, values dimmed |
| `WAITING` | no Mac found yet | see [Troubleshooting](#troubleshooting) |

**Watch over SSH** alongside the panel. The agent streams to every display it
finds, so both stay live:

```bash
ssh pi@192.168.0.148
macdash-tui --port 0      # --port 0 because the service already uses port 9999
```

**Several screens** work with no extra setup: install on another Pi.

**Reading the numbers:** the big CPU figure is the whole machine (0–100%).
App percentages count one core as 100%, so they can add up to more than the
big figure. Network rates are in bytes per second (`K` = 1024 bytes).

## How it works

There are two small programs: an **agent** on the Mac that measures, and a
**display** on the Pi that draws.

**Finding each other.** Every 3 seconds the Pi sends a tiny "I'm here" message
to a UDP multicast group (`239.255.42.99:9998`) on the local network. The
agent listens there and sends to wherever the messages come from. Nobody types
an IP address, and a Pi that reboots or gets a new address is found again
within seconds.

**Sending the numbers.** Five times a second the agent reads the Mac's stats
straight from macOS (CPU, memory, temperature, network, disk, top apps). It sends
them as one small JSON message in a single UDP packet to port 9999. There is no
connection to keep alive: if a packet is lost, the next one arrives 200 ms
later. Each packet is a complete snapshot, kept under 1400 bytes so the network
never has to split it. The Pi shows the data as `STALE` when packets stop
coming, and turns the screen off when the Mac sleeps.

**Drawing the screen.** The polished dashboard doesn't use the terminal at all.
The Pi has no desktop, browser or GPU toolkit. The display program draws every
pixel itself: rounded cards, smooth edges, charts and the Inter font, built
into the program. Then it writes the finished image straight into Linux's
framebuffer (`/dev/fb0`), which is the screen's memory. To stay fast on a
Pi Zero, it redraws only the parts of the screen that changed. When the
framebuffer isn't available (over SSH, or in a Mac terminal), the same data is
drawn as a text dashboard with [ratatui](https://ratatui.rs) instead.

## Troubleshooting

**The screen stays on `WAITING`.** Make sure the agent is running on the Mac
(`launchctl list | grep macdash`). If it is, your router is probably blocking
multicast. Reinstall with `--mac <your Mac's IP>`, or start the agent with
`--target <pi-ip>`.

**The screen is blank, with no dashboard and no login prompt.** This means an
install failed partway. To give the console back to a login prompt and try the
dashboard again:

```bash
./deploy/fix-pi-console.sh pi@192.168.0.148
```

**Bars and graphs show as `#`.** The console font failed to load. Reinstall
without `--font` to rebuild the default one.

**The layout is cramped or panels are missing.** The font is too large for the
panel. Reinstall with `--font Lat15-Terminus16`.

**Restart or inspect the service on the Pi:**

```bash
sudo systemctl restart macdash
systemctl status macdash macdash.timer
journalctl -u macdash -b
```

**Temperature is missing.** It uses a private macOS API that a future macOS
might break. Every other metric keeps working.
`macdash-agent --list-sensors` shows the raw sensor data.

**Check without a terminal:** `cargo run -p macdash-tui --example snapshot`
renders one frame as plain text, and `macdash-agent --once` prints one snapshot
as JSON.

## Reference

**`macdash-agent`** (Mac)

| Flag | |
|---|---|
| `--target <ip[:port]>` | Also stream to a fixed address (repeatable) |
| `--interval <ms>` | Sampling interval (default 200) |
| `--once` | Print one snapshot as JSON and exit |
| `--list-sensors` | Dump raw thermal sensors |
| `-v` | Log discovery and send counts |

**`macdash-tui`** (display)

| Flag | |
|---|---|
| `--theme <name>` | `classic`, `traffic`, `quiet`, `thermal`, `phosphor`, `sapphire`, `grey`, `graphite` |
| `--text` | Force the text dashboard, even on the Pi's console |
| `--hdmi` | HDMI off while the Mac's display sleeps (VC4/KMS, `fb0`) |
| `--backlight <path>` | Backlight to darken while the Mac sleeps |
| `--font <path>` | Console font to load before drawing |
| `--mac <ip>` | Also announce directly to this Mac |
| `--port <n>` | Listen port (default 9999; `0` picks any free port) |
| `--name <label>` | Name shown in the agent's logs |
| `--no-beacon` | Don't announce (use with the agent's `--target`) |

**Network:** 9998/udp for discovery (multicast `239.255.42.99`, sent by the Pi
every 3 s) and 9999/udp for stats (unicast, sent by the Mac every 200 ms). Both
stay on the local network.

## Development

```bash
cargo test --workspace
cargo run --release -p macdash-agent --example bench     # where the agent's time goes
```

The code is three crates: `proto` (wire format, shared by both sides), `agent`
(macOS) and `tui` (the display). [ANALYSIS.md](ANALYSIS.md) covers the design:
why UDP and beacons, how the CPU cost went from 22% to 0.012%, sleep detection
and the edge cases.

## Credits

The pixel dashboard uses [Inter](https://rsms.me/inter/) by Rasmus Andersson, under the SIL Open Font License (`crates/tui/assets/fonts/LICENSE-Inter.txt`). The fonts are embedded in the binary.

## License

MIT
