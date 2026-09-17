# macdash — Design & Analysis

**Date:** 2026-09-17
**Status:** Approved, implementing

Stream live system metrics from a Mac to a Raspberry Pi Zero W over the LAN,
rendered as a terminal dashboard on a small always-on screen.

---

## 1. Problem

Show Mac vitals — CPU, memory, network, temperature, top processes — on a
secondary always-on display driven by a Raspberry Pi Zero W, mounted under the
main monitor.

### Constraints

| Constraint | Source | Consequence |
|---|---|---|
| Mac cost ≤ ~0.5% of one core | User requirement | Collection budget is measured in microseconds; no shelling out to `powermetrics` or `ps` |
| Negligible LAN traffic | User requirement | Tens of kbps, not hundreds |
| Pi Zero W v1: ARMv6, 1 core, 512 MB | Hardware | No Node/Electron/Python runtime; static native binary |
| Both hosts DHCP, addresses may change | Network | No hardcoded IPs on either side |
| Headless Debian, no X | Pi OS | TUI on the framebuffer console |
| Read over SSH today, 7" panel later | Deployment | Responsive layout, not fixed-size |

### Non-goals

Internet access, historical storage, alerting, multi-Mac aggregation,
authentication. This is a LAN appliance on a trusted network.

---

## 2. Transport

### The insight that shrinks the problem

The Mac's IP address does not matter if the Mac is the one sending. Making the
Mac the pusher deletes half the discovery problem before any code is written:
only the Pi's address needs discovering, and only by one party.

### Options considered

**A — REST polling.** Pi issues `GET /stats` on a timer. Costs a TCP handshake
plus HTTP framing per sample, requires an HTTP server on the Mac and burns the
Pi's single core building requests. It still needs discovery, so it saves no
work there. At 5 Hz it is roughly 5× the overhead of the chosen design for no
benefit. Rejected.

**B — TCP or WebSocket with mDNS.** Pi resolves `mac.local` and holds a
long-lived connection. Reliable and ordered, but ordering and retransmission
are guarantees this workload does not need, and the price is a connection
state machine — backoff, half-open detection, a UI state for "connected but no
data" — plus an mDNS dependency on both ends. Rejected as unnecessary
machinery.

**C — UDP push with a Pi-side beacon.** Chosen.

### Chosen design

```
  Pi Zero W                                    Mac
      |                                         |
      |  ── beacon ─────────────────────────▶   |   multicast 239.255.42.99:9998
      |     every 3 s, ~30 bytes                |   (Mac learns Pi's address)
      |                                         |
      |  ◀──────────────────── stats JSON ──    |   unicast <pi>:9999
      |     every 200 ms, ~700 bytes            |   every 200 ms
```

The Pi announces itself to an administratively-scoped multicast group every
3 seconds. The Mac listens on that group, records the sender's address, and
unicasts the metric stream directly to it.

### Why this wins

**Discovery is self-healing and dependency-free.** DHCP reassigns the Pi's
address: the next beacon carries the new one and the Mac re-targets within
3 seconds. The Pi reboots: the beacon resumes, the stream resumes. The Mac
restarts: it relearns on the next beacon. No mDNS library, no config file
holding an address that goes stale, no manual step — on either side.

**Multicast is used only where it is cheap.** WiFi handles multicast poorly:
frames go out at the lowest basic rate and many access points rate-limit or
drop them. That is disqualifying for a 5 Hz data stream and irrelevant for a
3-second beacon that retries forever. The payload travels over ordinary
unicast, which WiFi handles well. Each mechanism is used where its weakness
does not apply.

**There is no connection state anywhere.** One `sendto` on the Mac, one
`recvfrom` on the Pi. Loss is self-correcting because every datagram is a
complete snapshot and the next one is 200 ms away.

**The beacon does not carry the Pi's IP address.** The Mac takes the source
address from `recvfrom`. A self-reported address is a guess when a host has
several interfaces; the packet's source address is ground truth.

**Multiple displays come free.** The Mac keeps a table of beacon sources seen
in the last 10 seconds and sends to all of them. A second screen is a second
Pi running the same binary — no configuration on the Mac.

### Cost at 5 Hz

| | |
|---|---|
| Wire | ~3.5 KB/s ≈ 28 kbps |
| Mac CPU | < 0.5% of one core (§4) |
| Pi CPU | Deserialize + redraw, idles the rest of the time |
| Dependencies | `std::net` on both ends |

### The tradeoff, stated plainly

UDP caps a datagram at ~1400 bytes before IP fragmentation. All metrics plus
five processes serialize to roughly 700 bytes, so it fits with room to spare —
but it is a real ceiling. A future "top 30 processes" view would need a second
packet type at a slower cadence, not a different transport.

---

## 3. Wire protocol

### Beacon — Pi → `239.255.42.99:9998`, every 3 s

```json
{"v": 1, "port": 9999}
```

### Stats — Mac → `<pi>:9999`, every 200 ms

```json
{
  "v": 1,
  "ts": 1789430400000,
  "host": "studio.local",
  "uptime_s": 481920,
  "cpu": { "total": 23.4, "user": 12.1, "sys": 11.3, "nice": 0.0,
           "load": [1.18, 2.29, 2.62], "ncpu": 10 },
  "mem": { "used": 12884901888, "total": 34359738368,
           "pressure": 41.2, "swap_used": 0, "swap_total": 0 },
  "net": { "iface": "en0", "rx_bps": 128400, "tx_bps": 9200,
           "rx_total": 1818477245, "tx_total": 108836948 },
  "temp": { "soc": 48.2, "gpu": 45.1 },
  "procs": [ { "pid": 842, "name": "WindowServer",
               "cpu": 8.2, "mem": 412000000 } ]
}
```

### Rules

**Every datagram is a complete snapshot, never a delta.** This is the decision
that makes UDP correct here rather than a compromise. A lost packet costs
200 ms of staleness and repairs itself with no sequence numbers, no
retransmission, no reconnect logic. Statelessness on the wire is what buys
statelessness in the code.

**`v` is checked, unknown fields are ignored.** Mismatched major version is
dropped with a visible warning rather than silently misrendered. Serde's
default tolerance means a newer Mac can add fields without breaking an older
Pi.

**Nullable metrics degrade in place.** `temp` is `null` when the thermal
sensors are unreadable; the rest of the packet is unaffected. One failing
source never takes the stream down.

**Two constraints are enforced in code, not documentation:**

- Process names truncate to 24 characters. Unbounded names are the only field
  that could push a packet past the MTU.
- `proto` carries a test that serializes a worst-case packet — maximum-length
  host and process names — and asserts the result is under 1400 bytes. Adding
  a field that breaks the budget fails the build.

**JSON, deliberately.** It is directly inspectable with `nc -ul 9999` during
debugging, which matters more here than the ~40% a binary encoding would save
on an already-negligible 28 kbps. Because `proto` owns both encode and decode,
switching to CBOR later is a one-file change.

---

## 4. Meeting the CPU budget

This constraint shapes the agent's structure, so it gets its own section.

CPU, memory, network and thermal are a handful of Mach and sysctl calls —
roughly 200 µs per sample. At 5 Hz that is about 0.1% of one core.

**Top processes is a different animal.** `proc_listpids` followed by a
`proc_pidinfo` and `proc_name` per PID is 1000+ syscalls on a typical Mac —
around 4 ms. At 5 Hz that would be 20 ms/s of pure syscall overhead, and it is
the only thing here that could plausibly register in Activity Monitor.

**So the agent runs two clocks:**

| Group | Rate | Cost/sample | Cost/s |
|---|---|---|---|
| cpu, mem, net, temp | 5 Hz | ~200 µs | ~1 ms |
| processes | 1 Hz | ~4 ms | ~4 ms |

The process table is cached and included in **every** packet, refreshed on
every 5th tick. The duplicated bytes are free at this bitrate, and the Pi's
receive path stays a straight deserialize with no "did this packet include
processes?" branch. A top-5 list refreshing at 1 Hz is visually
indistinguishable from one refreshing at 5 Hz, and the split removes 80% of
the agent's dominant cost.

**Total: ~5 ms of CPU per second, under 0.5% of one core.**

---

## 5. Correctness details

### Counter deltas

CPU percentage and network throughput both derive from monotonically
increasing counters, so each value is `(now − prev) / elapsed`. Three cases
must be handled explicitly:

- **First sample** has no predecessor. Reports `null`, not a fabricated zero.
- **Sleep/wake.** After suspend, `elapsed` is hours while the counters barely
  moved — or moved enormously. Either way the delta is meaningless. A gap over
  2 seconds discards the sample and reseeds the baseline. Without this you get
  a garbage spike on every wake, which is precisely when you are looking at
  the screen.
- **Counter reset** (interface reconfigured). A negative delta is treated as a
  reseed rather than wrapping to a huge positive number.

### The private thermal API is quarantined

Apple Silicon exposes no documented unprivileged temperature interface.
`powermetrics` requires root. The unprivileged path is
`IOHIDEventSystemClient`, matching on usage page `0xff00` / usage `0x0005` and
reading `kIOHIDEventTypeTemperature` — the approach used by `macmon` and
Stats.app.

It is undocumented and may change in any macOS release, so it is confined to
one module behind a `Result`. Failure yields `"temp": null` while every other
metric continues. A private API is allowed to break; it is not allowed to take
the agent down with it. `macdash-agent --list-sensors` dumps the raw sensor
names to make future breakage diagnosable in seconds.

### Interface selection

"Which interface is the network one" has no static answer on a Mac with
Ethernet, WiFi, VPNs and bridges. The agent opens an unconnected UDP socket,
`connect`s it to the target (no packets are sent — `connect` on UDP only sets
the default destination), and reads back the local address the kernel chose.
That address is matched to an interface name via `getifaddrs`. This asks the
routing table the same question the traffic will ask, which is the only source
of truth. Re-evaluated every 5 seconds so a VPN or cable change is picked up.

### Presence is derived, never signalled

The Pi tracks time since the last packet. There is no heartbeat message type
and no connection concept — just arithmetic on a timestamp:

| Elapsed | State | Display |
|---|---|---|
| < 1 s | `Live` | Normal |
| 1–10 s | `Stale` | Last values dimmed, warning in status bar |
| > 10 s | `Asleep` | "Mac asleep", backlight off (§6) |

Any arriving packet resets to `Live` immediately. Flapping is impossible
because receiving a packet is unambiguous and the asleep threshold is long.

---

## 6. Sleep detection and backlight control

**Requirement:** when the Mac sleeps, drive the 7" panel's backlight to 0;
restore it when the Mac wakes.

**This requires no protocol change.** The Pi already computes time since last
packet for the staleness indicator. Sleep detection is the same clock against
a longer threshold — a third arm on an `if` that already exists. The Mac stops
transmitting the moment it suspends, and resumes within one 200 ms tick of
waking, so the wake path is effectively instant.

Backlight control writes to the standard Linux sysfs interface:

```
/sys/class/backlight/<device>/brightness      # 0 = off
/sys/class/backlight/<device>/max_brightness  # restore target
```

The Pi records the pre-sleep value and restores that, rather than assuming
maximum — the brightness you chose is the brightness you get back.

Built now, disabled by default, enabled with `--backlight <path>`. With no
flag it is a no-op, so the same binary runs correctly on a Pi with no panel
attached. The state machine and its tests ship in v1; pointing the flag at the
device when the screen arrives is the entire installation step.

### Known limitation

At the protocol level, a WiFi dropout is indistinguishable from a sleeping
Mac. Both stop the stream; both darken the screen after 10 seconds. For this
appliance that behaviour is acceptable — a dark screen is the right answer to
"no data" either way.

The v2 fix is an explicit farewell: registering with `IORegisterForSystemPower`
lets the agent transmit a `{"v":1,"bye":"sleep"}` datagram during the sleep
callback, making the two cases distinguishable and the response instant. It is
deferred because it adds a CoreFoundation run loop to the agent for a
refinement of an already-working mechanism. Note that a farewell is
best-effort by nature, so the timeout path remains the mechanism of record
regardless.

---

## 7. Architecture

```
crates/proto/   Wire types, version constant, presence state machine
      │         Zero platform dependencies. Compiles for macOS and ARMv6.
      ├──────────────┬──────────────
      ▼              ▼
crates/agent/   crates/tui/
  macOS only      Linux/macOS
  Mach + IOKit    ratatui + crossterm
  → the Mac       → the Pi
```

Rust on both ends, for one reason that outweighs preference: the target is
ARMv6 with 512 MB and no package manager worth using. A statically-linked musl
binary is a single ~3 MB file that is copied and run. No runtime, no
dependency resolution on-device, no Node-on-ARMv6 archaeology.

The shared `proto` crate means encoder and decoder are compiled from the same
struct definitions. A field renamed on one side is a compile error, not a
`null` that appears on screen three days later.

### Module layout

```
proto/   packet.rs     StatsPacket, Beacon, Cpu, Mem, Net, Temp, Proc
         presence.rs   Presence enum + pure-function tracker

agent/   discovery.rs  Multicast listener, target table with TTL
         sender.rs     Serialize + unicast fan-out
         collect/      cpu · mem · net · temp · procs, one concern each
         ffi.rs        Mach, libproc and IOKit declarations, isolated

tui/     receiver.rs   UDP listener → channel
         beacon.rs     3 s multicast announcer
         state.rs      Ring buffers, presence tracking
         brightness.rs sysfs backlight writer
         ui/           ratatui rendering, responsive layout
```

---

## 8. Presentation

`ratatui` + `crossterm`. The layout is responsive rather than fixed: the same
binary must be readable in an SSH window today and on a 1024×600 panel
(~128×37 characters) later, without a rewrite in between. Below 60 columns it
drops to a single stacked column.

Sparklines for CPU and network history, gauges for memory, a table for
processes. The Pi holds a 120-sample ring buffer in memory for the sparklines
— roughly 4 minutes of history. **Nothing is written to the SD card**; a
dashboard that writes continuously to flash is a dashboard that eventually
kills its own storage.

---

## 9. Testing

The valuable tests here are about pure logic, not hardware:

| Test | What it protects |
|---|---|
| Delta math | Wraparound, divide-by-zero on identical timestamps, the sleep gap. This is where real bugs live, and none of it needs a Mac. |
| Packet size budget | The < 1400 byte MTU guarantee, with worst-case names |
| Proto round-trip | Serialize/deserialize equality; decoding a *newer* packet without panicking |
| Presence transitions | Live → Stale → Asleep → Live, including the backlight hook |
| Sensor plausibility | Integration tests asserting ranges on real hardware |
| TUI layout | Render to `TestBackend` at 128×37 and 40×10; no panic, no overflow |
| Loopback end-to-end | Agent → 127.0.0.1 → receiver, validated |

---

## 10. Deployment

**Cross-compilation.** `arm-unknown-linux-musleabihf` via `cross` (Docker).
ARMv6 is the reason a container is needed rather than plain rustup — the
prebuilt musl toolchain targets ARMv7 by default and produces binaries that
fault on a Zero W v1.

**Pi:** systemd unit bound to `tty1`, `Restart=always` — the dashboard comes up
on boot with no keyboard attached.

**Mac:** launchd agent in `~/Library/LaunchAgents` with `KeepAlive`, surviving
logout and reboot. Runs as the user; no privileged component anywhere in the
system.

---

## 11. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Thermal private API changes in a macOS release | Medium | Quarantined behind `Result`; `temp: null` degrades gracefully; `--list-sensors` makes diagnosis immediate |
| ARMv6 cross-compilation friction | Medium | Proven early rather than at the end; musl static linking avoids glibc version skew entirely |
| AP blocks multicast between clients | Low | Client isolation would break discovery; `--target <ip>` escape hatch bypasses the beacon |
| Process CPU timebase units differ from assumption | Medium | Converted via `mach_timebase_info` and verified empirically against `top` |
