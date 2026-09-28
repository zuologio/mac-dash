<div align="center">

# macdash

**Your Mac's vitals, live on a second screen.**

A metrics agent for macOS and a terminal dashboard for a Raspberry Pi, talking
over your LAN. Point a small display at it and leave it running.

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
│                                                                        ││██████████████████████████                          │
│                                                                        ││██████████████████████████                          │
│                                                                        │└────────────────────────────────────────────────────┘
│                                                                        │┌ THERMAL ───────────────────────────────────────────┐
│                                                                        ││ 41.4°C  SoC                                        │
│                                                                        ││ 32.0°C  batt                                       │
│                                                                        ││                                                    │
│                                                                        ││▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁                          │
│                                                                        ││██████████████████████████                          │
│                                                                        │└────────────────────────────────────────────────────┘
│                                                                        │┌ NETWORK ───────────────────────────────────────────┐
│▅                                                                       ││▼ 5.0K/s   ▲ 5.0K/s                                 │
│█                                                                       ││█                                                   │
│█       ▁                                                               ││█▂▂ ▆▆▄   ▂▂     ▄▄▂▂▄▄▂▂▂                          │
│█     ▆ █            ▆▆                                                 ││█                                                   │
│█▄▄ ▅▇█ █▁   ▁   ▇▇▁▁██▅▅▁                                              ││█▂  ▂      ▂ ▂▂  ▂▂▂▂▄▄▂▂▂                          │
│███▄███▃██▄▇▆█▄▄▄█████████                                              ││session  ▼ 379M   ▲ 2.9G                            │
└────────────────────────────────────────────────────────────────────────┘└────────────────────────────────────────────────────┘
┌ TOP PROCESSES ───────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│Docker Desktop Helper (R    25.3%  92.8M  ▰▰▰▰▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│claude                      14.9%   261M  ▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│opencode                     5.0%   161M  ▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│Code                         4.2%   107M  ▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│Code Helper                  3.8%  58.8M  ▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│com.apple.Virtualization     3.6%   406M  ▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│com.docker.backend           2.6%   106M  ▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│opencode                     2.1%   118M  ▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱                                            │
│                                                                                                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
 LIVE en0 26 pkts  q quit
```

<div align="center"><sub>Live output, unedited.</sub></div>

---

## What it costs

Measured on an M5 with `proc_pid_rusage`, the kernel's own nanosecond CPU
accounting:

| | |
|---|---|
| **Mac CPU** | **0.012%** of one core — 7.4 ms per minute |
| **Network** | **37 kbps** — 900-byte snapshots at 5 Hz |
| **Mac memory** | 5.4 MB resident |
| **Pi binary** | 695 KB, statically linked, no runtime |

`ps` will tell you 0.5%. It rounds a few milliseconds up to a full 10 ms tick;
the number above is the real one.

## How the two ends find each other

Both machines are on DHCP, so neither address is stable. Rather than pinning
one down, the Pi announces itself and the Mac listens:

```
  Pi Zero                                      Mac
     │                                          │
     │  ── "I'm here" ──────────────────────▶   │   multicast, every 3 s
     │                                          │
     │  ◀────────────────────── stats JSON ──   │   unicast, every 200 ms
     │                                          │
```

The Mac takes the Pi's address from the packet's source, never from its
contents — a host with several interfaces will misreport its own address, but
the source address is always the truth.

**Everything recovers on its own.** New DHCP lease, Pi reboot, Mac restart, WiFi
blip: the next beacon repairs it within 3 seconds. There is no config file
holding an address that goes stale, no mDNS dependency, and no connection state
machine on either side.

**Every datagram is a complete snapshot, never a delta.** That is what makes UDP
the right choice here rather than a compromise — a lost packet costs 200 ms of
staleness and fixes itself, so there is nothing to retransmit, sequence, or
reconnect.

Multiple displays work with no extra configuration: the Mac streams to every
beacon it hears. A second screen is a second Pi running the same binary.

## Quick start

**On the Mac:**

```bash
git clone <your-repo-url> && cd macdash
./deploy/install-mac.sh          # builds + installs a launchd agent
```

**On the Pi** (needs Docker on the Mac for the ARMv6 toolchain):

```bash
cargo install cross --git https://github.com/cross-rs/cross
./deploy/build-pi.sh                        # --zero2 for a Pi Zero 2 W
./deploy/install-pi.sh pi@192.168.0.148
```

That's it — the dashboard comes up on `tty1` at boot, with no keyboard
attached, and finds the Mac by itself.

To try it before installing anything, run both halves by hand:

```bash
cargo run --release -p macdash-agent      # on the Mac
cargo run --release -p macdash-tui        # anywhere on the same LAN
```

The installer preserves an existing custom console font unless you pass
`--font` or `--ttf`. Font bitmaps and their Unicode map are loaded together
before the first frame. Diagnostics go to the journal, not to the dashboard.

For the **7-inch 1024×600 display**, use `Lat15-Terminus16`: its 8×16-pixel
cells give a 128×37 grid, enough for CPU, memory, thermal, network and all
eight processes. This is the default for new installations. The larger
16×32 font used on a 1080p monitor gives only 64×18 on this panel, triggering
the reduced layout.

To resize an existing installation without restarting the dashboard, copy
`deploy/fit-7-inch-display.sh` and `deploy/make-console-font.py` into the same
directory on the Pi, then run `sudo bash fit-7-inch-display.sh` there. It
loads the smaller patched font live, preserves history, and saves the font
for future boots. The previous installed font is backed up as
`/usr/local/share/consolefonts/macdash.psf.before-7-inch`.

At boot, `macdash.timer` queues the service after `multi-user.target`,
`cloud-final.service` and console setup have finished. The timer avoids an
ordering cycle with cloud-init. Restart the display with
`sudo systemctl restart macdash`; inspect both units with
`systemctl status macdash macdash.timer`.

The display learns the agent's current address from incoming stats and sends
beacons directly there as well as to multicast and any configured `--mac`
address. This keeps a stale DHCP address in the installer configuration from
interrupting an established stream. The Pi setup also disables Wi-Fi power
saving for its active wireless connection without reconnecting it.

## Watching over SSH

The dashboard is just a terminal program, so it runs in an SSH session as
easily as on the panel:

```bash
ssh pi@192.168.0.148
macdash-tui --port 0        # --port 0 if the service is already running
```

`--port 0` matters once you have installed the service: it already holds 9999,
and the OS picks a free port instead. The agent streams to **every** display it
discovers, so your SSH view and the panel both show live data at the same time
— which also makes this the easy way to check the system before the screen
arrives.

Press `q` to quit; the service keeps running.

To check it works without a terminal at all — over a pipe, or from a script —
render a single frame as plain text:

```bash
cargo run -p macdash-tui --example snapshot
```

## When the Mac sleeps

The dashboard already knows how long it has been since the last packet — that
is what the staleness indicator is. Sleep detection is the same clock read
against a longer threshold:

| Signal | State | Screen |
|---|---|---|
| packets flowing | `LIVE` | normal |
| Mac's display asleep | `DISPLAY OFF` | **backlight off**, values current |
| silence 1–10 s | `STALE` | values dimmed; an already sleeping display stays off |
| silence > 10 s | `ASLEEP` | **backlight off**, values dimmed |

A Mac on AC power is often set never to sleep, in which case the silence this
once relied on never arrives — the screen goes dark, you walk away, and the
panel stays lit all evening. The agent reports its display state directly, so
the panel follows the screen rather than the machine.

A packet reporting the Mac's display awake restores the output. Packets that
still report display sleep keep it off. Older agents without a display-state
field restore it on any packet. The dashboard keeps receiving and recording
metrics while the screen is off.

For an HDMI panel on the Pi's **VC4/KMS framebuffer console**, enable HDMI
standby:

```bash
./deploy/build-pi.sh
./deploy/install-pi.sh <user@pi-host> --hdmi
```

This writes `4` (power down) or `0` (restore) to
`/sys/class/graphics/fb0/blank`. The LAFVIN 7-inch HDMI panel was checked with
this method: it goes fully dark and the dashboard returns when output resumes.
The panel may show “No signal” before its own standby delay expires. HDMI
standby does not set a brightness percentage: set the panel's physical
brightness control to 100% if that is the level you want when it wakes.

The dashboard skips drawing while HDMI is off, repaints on wake, and retries
failed power transitions once a second. Output is restored on exit; the
installer also adds a systemd stop action to restore it after a crash or
service stop. This option targets `fb0` and requires write access to its
`blank` file (the installed service runs as root).

For a panel with a Linux backlight device, use:

```bash
./deploy/install-pi.sh pi@192.168.0.148 --backlight /sys/class/backlight/rpi_backlight
```

The backlight option restores the brightness you had set, not the maximum.
Without either option, display power control is disabled. Pass `--hdmi` again
on subsequent installations to keep HDMI standby enabled.

One honest limitation: a WiFi dropout looks exactly like a sleeping Mac. For a
screen on your desk, a dark panel is the right answer to both.

## What it shows

**CPU** total and per-core, user/system split, load averages against core
count, 4 minutes of history · **Memory** used/total, macOS memory pressure,
compressed, swap against its limit, with history · **Network** up/down
throughput and session totals on the interface that actually carries the
traffic · **Thermal** SoC and battery temperature with history
· **Processes** top 8 by CPU · **Clock** taken from the Mac, because a Pi has
no battery-backed clock of its own

Temperature on Apple Silicon has no documented unprivileged interface —
`powermetrics` wants root. macdash reads the same private `IOHIDEventSystemClient`
API that macmon and Stats use, so **nothing here needs sudo**. It is quarantined
behind a `Result`: if a future macOS breaks it, temperature reads `null` and
every other metric keeps flowing. `macdash-agent --list-sensors` dumps the raw
sensor names when you need to see why.

## Getting to 0.012%

The first working version cost **22% of a core** — 1800× the final figure. Not
from the metrics you would suspect:

```
temp     44660 µs   ← 99.6% of the time
procs      959 µs
net         43 µs
cpu         12 µs
mem          2 µs
```

Temperature was the entire budget. A Mac exposes 45 thermal sensors and each
read is a blocking ~500 µs IOKit round trip, so polling all of them at 5 Hz
cost 44 ms per sample — a thousand times everything else combined. The process
table, which the design had predicted would dominate, was never the problem.

Two changes fixed it. **Read 5 sensors, not 45**: the dashboard displays one SoC
number, so reading 23 die sensors to take their maximum spends 22 round trips on
resolution nobody sees. **Read them every 2 seconds, not 5 times a second**: a
CPU die is a thermal mass, and the heatsink it is bonded to takes tens of
seconds to move. A 2-second-old temperature is indistinguishable on screen from
a fresh one.

The lesson worth keeping: the expensive metric was the one nobody thought to
measure, and it was invisible until the whole loop was profiled rather than
reasoned about.

## Design

```
crates/proto/   wire types + presence state machine   ──┐
crates/agent/   → macOS: Mach, IOKit, libproc         ─┤  both compile
crates/tui/     → Pi: ratatui + crossterm             ─┘  against proto
```

Rust on both ends for one reason that outweighs preference: the target is ARMv6
with 512 MB and no package manager worth using. A static musl binary is a single
file you copy and run.

The shared `proto` crate means encoder and decoder come from the same struct
definitions — a field renamed on one side is a compile error, not a `null` that
shows up on screen three days later. It also carries a test that serializes a
worst-case packet and fails the build if it exceeds 1400 bytes, so the "one
datagram, no fragmentation" guarantee is enforced rather than documented.

[ANALYSIS.md](ANALYSIS.md) has the full design: transport alternatives and why
they lost, the counter-delta edge cases, and the reasoning behind each
constant.

## If the screen is blank

The dashboard and a login prompt cannot both own tty1, so the installer masks
`getty@tty1` — but only *after* confirming macdash started. If that ordering is
broken (or an install fails midway), tty1 ends up with nothing attached: no
dashboard, no prompt, just the boot log and then silence. The Pi is still fine
over SSH; only the attached screen is dead.

```bash
./deploy/fix-pi-console.sh g10@192.168.0.148
```

That gives the console back to a login prompt, tries macdash again, and hands
tty1 over only if the service actually comes up.

## Gotchas found the hard way

**macOS multicast needs a per-interface join.** Joining via `INADDR_ANY` binds
to one kernel-chosen interface. With a VPN up, that is often not your LAN — and
the failure is silent: the socket binds, the join succeeds, and no packet ever
arrives. Both sides now join and send per-interface.

**A VPN can capture the default route.** The agent asks the routing table which
interface reaches the *display*, then prefers a physical one over a `utun`
tunnel — otherwise it reports the tunnel's near-zero counters on a busy machine.

**Counters lie across sleep.** After suspend, elapsed time is hours while the
counters barely moved. Any gap over 2 seconds is discarded and the baseline
reseeded, which is why you do not get a garbage spike on the graph at exactly
the moment you walk back to your desk.

**Never mask getty before the replacement works.** Masking `getty@tty1` and
starting the dashboard look like one step, but if the second half fails the
console has no owner at all — and the symptom (a blank screen) gives no hint
that a login prompt was disabled. Confirm the new owner is running, then take
the console away.

**Mach ticks are not nanoseconds.** On Apple Silicon the timebase is 125/3.
Treating raw ticks as nanoseconds overstates every process's CPU by ~41×.

## Tests

```bash
cargo test --workspace     # 78 tests
```

The valuable ones are pure logic and need no hardware: counter-delta edge cases
(wraparound, zero elapsed, the sleep gap), the packet size budget, presence
transitions including the backlight, and TUI layout rendered at sizes from
128×37 down to 1×1 — because a layout panic on a headless Pi with no keyboard
means a power cycle.

```bash
cargo run --release -p macdash-agent --example bench      # where the time goes
cargo run --release -p macdash-agent --once               # one snapshot as JSON
cargo run --release -p macdash-tui --example snapshot     # one frame as text
```

## Reference

**Agent** `--target <ip>` fixed destination · `--interval <ms>` sample rate
· `--once` one JSON snapshot · `--list-sensors` raw thermal dump · `-v` log
discovery

**Display** `--hdmi` HDMI standby on sleep (VC4/KMS, fb0) ·
`--backlight <path>` panel to darken on sleep · `--name <label>`
identify in agent logs · `--port <n>` listen port · `--no-beacon` disable
announcements

**Ports** 9998/udp beacon (multicast 239.255.42.99) · 9999/udp stats (unicast).
Both TTL-limited to the local network.

## License

MIT
