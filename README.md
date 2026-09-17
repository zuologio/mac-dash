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
 ● macAbre.local  up 16h 19m
┌ CPU ───────────────────────────────────────────────────────────────┐┌ MEMORY ────────────────────────────────────────────────┐
│ 36.0%  usr 19.5%  sys 16.5%                                        ││12.8G / 16.0G                                           │
│████████████████████████▌···········································││████████████████████████████████████████████▉···········│
│cores ▆▅▄▃▃▃▂▃▂▃                                                    ││pressure 65%  swap 6.4G                                 │
│                                                                    │└────────────────────────────────────────────────────────┘
│     ▁                                                              │┌ THERMAL ───────────────────────────────────────────────┐
│██▇▆▇██▇▇▇█▇▇▇▁▆▆      ▃                                            ││ 58.3°C  SoC                                            │
│█████████████████▇▇█████                                            ││ 32.0°C  batt                                           │
│                                                                    ││                                                        │
└────────────────────────────────────────────────────────────────────┘└────────────────────────────────────────────────────────┘
┌ TOP PROCESSES ─────────────────────────────────────────────────────┐┌ NETWORK ───────────────────────────────────────────────┐
│pgAdmin 4 Helper (Render                               79.3%   425M ││▼ 4.9K/s   ▲ 4.9K/s                                     │
│Google Chrome Helper                                   18.7%  84.3M ││   █ █                                                  │
│Code Helper (Renderer)                                 16.9%   115M ││   █▇█                                                  │
│Google Chrome Helper (Re                               16.8%   210M ││ █ ███     ██ █                                         │
│com.apple.Virtualization                               10.7%   426M ││████████████████████████                                │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││██         █   ██                                       │
│                                                                    ││████▇███████████████████                                │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
│                                                                    ││                                                        │
└────────────────────────────────────────────────────────────────────┘└────────────────────────────────────────────────────────┘
 LIVE en0 24 pkts  q quit
```

<div align="center"><sub>Live output. The <code>yes</code> processes are load I generated to make the graphs interesting.</sub></div>

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

| Silence | State | Screen |
|---|---|---|
| < 1 s | `LIVE` | normal |
| 1–10 s | `STALE` | values dimmed |
| > 10 s | `ASLEEP` | **backlight off** |

Any arriving packet restores it, so waking the Mac lights the panel within one
200 ms tick. It needed no protocol support at all.

Enable it when your screen arrives:

```bash
./deploy/install-pi.sh pi@192.168.0.148 --backlight /sys/class/backlight/rpi_backlight
```

The Pi restores the brightness you had set, not the maximum. Without the flag
the whole mechanism is a no-op, so the same binary runs fine on a Pi with no
panel.

One honest limitation: a WiFi dropout looks exactly like a sleeping Mac. For a
screen on your desk, a dark panel is the right answer to both.

## What it shows

**CPU** total and per-core, user/system split, load averages, 4 minutes of
history · **Memory** used/total, macOS memory pressure, swap, compressed
· **Network** up/down throughput on the interface that actually carries the
traffic · **Thermal** SoC and battery temperature · **Processes** top 5 by CPU

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

**Display** `--backlight <path>` panel to darken on sleep · `--name <label>`
identify in agent logs · `--port <n>` listen port · `--no-beacon` disable
announcements

**Ports** 9998/udp beacon (multicast 239.255.42.99) · 9999/udp stats (unicast).
Both TTL-limited to the local network.

## License

MIT
