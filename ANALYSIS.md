# macdash — Design & Analysis

**Date:** 2026-09-17
**Status:** Implemented and verified

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
eight processes serialize to **1361 bytes in the worst case** — maximum-length
host and process names, 24 cores, every optional field present. That is the
real ceiling and it is close: a ninth process would cost ~80 bytes more and
exceed it.

A *typical* packet is only ~900 bytes, which makes the headroom look far
larger than it is. The budget test measures the worst case deliberately: a
packet that fits on an idle Mac and fragments under load is precisely the bug
it exists to prevent. `process_count_is_at_the_budget_ceiling` holds the
constant from the other side, failing if another process would still fit, so
neither direction can drift unnoticed.

A "top 30 processes" view would need a second packet type at a slower cadence,
not a bigger packet.

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

> **Revised after measurement.** The predictions in this section were wrong,
> and the error is instructive enough to keep visible rather than quietly
> correct. The original text is preserved below the measured results.

### What was predicted

Process enumeration would dominate: `proc_listpids` plus a `proc_pidinfo` per
PID is 1000+ syscalls, so it was budgeted at ~4 ms and moved to a 1 Hz clock,
with everything else — including temperature — assumed to cost ~200 µs
together.

### What was measured

Profiling the real loop (`cargo run -p macdash-agent --example bench`) on an
M5:

```
temp     44660 µs   <- 99.6% of the total
procs      959 µs
net         43 µs
cpu         12 µs
mem          2 µs
```

**Temperature was the entire cost, at 1000x everything else combined.** A Mac
exposes 45 thermal sensors, and each `IOHIDServiceClientCopyEvent` is a
blocking IPC round trip of roughly 500 µs. Reading all 45 at 5 Hz cost 44 ms
per sample: 22% of a core.

Process enumeration — the predicted problem — was never significant. The 1 Hz
split is still worth keeping, but it was solving the smaller issue.

### What fixed it

Two changes, both following from the same observation: the dashboard displays
*one* SoC temperature.

1. **Poll 5 sensors, not 45.** Reading 23 die sensors to take their maximum
   spends 22 blocking round trips on resolution nobody can see. Adjacent die
   sensors on one package track within a degree or two, so a small fixed
   subset finds the hot spot just as well. Selection happens once at startup
   and is stable, so the reported figure does not jump as sensors enter and
   leave the set.

2. **Read every 2 s, not every 200 ms.** Temperature is a thermal mass. A die
   under sudden full load takes seconds to climb and the heatsink tens of
   seconds, so a 2-second-old reading is indistinguishable on screen from a
   fresh one. Every packet still carries a temperature.

### Final duty cycle

| Group | Rate | Cost/sample |
|---|---|---|
| cpu, mem, net, uptime | 5 Hz | ~60 µs |
| processes | 1 Hz | ~750 µs |
| temperature (hardware) | 0.5 Hz | ~2200 µs |
| display state (hardware) | 0.5 Hz | ~15-28 µs |

**Measured: 0.012% of one core** — 7.4 ms of CPU per minute of wall clock, via
`proc_pid_rusage`. Note that `ps` reports this as 0.5%: it accounts in 10 ms
ticks and rounds a few milliseconds up to a whole one. Any measurement of a
process this cheap needs nanosecond accounting to mean anything.

### Display state pays the same toll

`CGDisplayIsAsleep` costs 15-28 µs per call in this process — trivial beside
the thermal read above, and not trivial against a budget of 7.4 ms per minute.
At the 5 Hz packet rate it would add ~130 µs per second; on the 2-second tick
it adds ~13 µs, an order of magnitude less.

The reasoning is the same one that throttles temperature, and it is worth
stating because the call *looks* free: a display that turned off two seconds
ago is indistinguishable on screen from one that turned off now. The bench
example prints both figures side by side so the decision stays visible to
whoever reads the numbers next.

### The lesson

The expensive metric was the one nobody thought to measure. Predicting cost
from syscall *counts* was the error — one blocking IPC round trip outweighed a
thousand cheap syscalls. Profiling the whole loop, rather than reasoning about
its parts, was the only thing that surfaced it.

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

One state is *not* derived this way. `DisplayOff` comes from a flag the agent
sends, because the thing it reports — the Mac's screen turning off while the
Mac keeps running — produces no gap in the packet stream to measure. See §6.

---

## 6. Sleep detection and backlight control

**Requirement:** when the Mac sleeps, drive the 7" panel's backlight to 0;
restore it when the Mac wakes.

**System sleep requires no protocol change.** The Pi already computes time
since last packet for the staleness indicator. Sleep detection is the same
clock against a longer threshold — a third arm on an `if` that already exists.
The Mac stops transmitting the moment it suspends, and resumes within one
200 ms tick of waking, so the wake path is effectively instant.

**Display sleep does, and the reason is worth recording.** A Mac on AC power
is commonly configured never to sleep (`pmset -c sleep 0`), and then the
silence this whole mechanism rests on never arrives: the screen goes dark, the
user walks away, and the panel stays lit indefinitely. The event that actually
means "nobody is looking" leaves no trace in the packet stream, so no amount of
cleverness on the Pi can infer it. The agent reads `CGDisplayIsAsleep` and
sends it.

`DisplayOff` sits beside `Asleep` rather than replacing it, because the two
differ in what they imply about the data on screen. `Asleep` means there is
nothing left to show, so the last values are dimmed to mark them stale.
`DisplayOff` means the data is still arriving and still correct — only nobody
is there to see it — so the readings render normally and just the backlight
goes out.

Silence outranks the flag. Once packets stop, `display_asleep` is a stale claim
about a Mac that can no longer be reached, so the elapsed-time states take over
regardless of what the last packet said.

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
| AP blocks multicast between clients | Low | Client isolation would break discovery; `--target <ip>` escape hatch bypasses the beacon |
| **macOS multicast join binds one interface** | **Hit during implementation** | Joining via `INADDR_ANY` alone silently receives nothing when a VPN holds the default route — the socket binds and the join succeeds, so the failure is invisible. Both sides now join and send per-interface. |
| **VPN captures the default route** | **Hit during implementation** | The route lookup preferred a `utun` tunnel over the physical LAN, reporting near-zero traffic on a busy machine. Physical interfaces are now preferred. |
| Process CPU timebase units differ from assumption | Medium | Converted via `mach_timebase_info` and verified empirically against `top`. Confirmed: the M5 timebase is 125/3, so raw ticks would have overstated CPU by ~41x |
| ARMv6 cross-compilation friction | Resolved | Verified: `readelf -A` reports `Tag_CPU_arch: v6` with zero ARMv7-only instructions, statically linked, 695 KB |
