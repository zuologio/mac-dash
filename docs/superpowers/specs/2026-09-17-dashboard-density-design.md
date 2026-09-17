# Dashboard density — design

**Date:** 2026-09-17
**Status:** Approved, not yet implemented

Fill the empty space on the macdash display without breaking the
single-datagram guarantee or the Mac's CPU budget.

---

## 1. Problem

The dashboard renders **73.9% blank cells** at its target size of 128×37 (a 7"
1024×600 panel at the default 8×16 console font). The two bottom panels are
~92% empty each:

```
TOP PROCESSES   rows 13-36   91.9% blank    24 rows allocated, 5 processes shown
NETWORK         rows 13-36   91.8% blank    24 rows allocated, graphs capped at 4
```

This is a layout problem, not a data shortage. Two independent causes:

1. **`TOP_PROC_COUNT = 5`** is fixed in the protocol regardless of how much
   panel height is available.
2. **Network sparklines are capped at 4 rows** (`widgets.rs`, the `gh`
   calculation) — deliberately and correctly, because a 2 KB/s fluctuation
   stretched over 20 rows reads as a dramatic cliff. Nothing fills the rows
   that cap frees.

Separately, **seven fields already cross the wire and are never displayed**:
`cpu.load[3]`, `cpu.ncpu`, `cpu.nice`, `mem.compressed`, `mem.swap_total`,
`net.rx_total`, `net.tx_total`, plus `proc.pid`. Showing them costs zero
bandwidth and zero Mac CPU.

### Constraints

| Constraint | Source | Consequence |
|---|---|---|
| Packet < 1400 bytes worst-case | `MAX_PACKET_BYTES`, enforced by test | Process count is capped; see §2 |
| Mac cost ≤ 0.012% of a core | ANALYSIS.md §4 | No new collection without measurement |
| Pi Zero W: ARMv6, 1 core | Hardware | Render cost must not grow materially |
| Target is 128×37, not the 49" dev monitor | Deployment | Judge every layout at 128×37 |
| Pi has no RTC | Hardware | Local clock is unreliable; see §4 |

### Non-goals

New metrics requiring new collection (disk I/O, GPU load, battery charge,
power draw). Scrolling or interactive process lists. Any change to the
transport, discovery, or presence model.

---

## 2. The packet budget ceiling

**The typical packet is misleading and must not be used for this decision.**

A live packet is ~903 bytes compact, suggesting ~497 bytes of headroom and room
for a dozen more processes. The enforcing test measures the **worst case** —
64-char host, 24 cores, 24-char process names, maximum values — and there the
picture is different:

```
TOP_PROC_COUNT= 5 ->  1099 bytes   OK       (today)
TOP_PROC_COUNT= 8 ->  1339 bytes   OK       <- the ceiling
TOP_PROC_COUNT=10 ->  1499 bytes   OVER BUDGET
TOP_PROC_COUNT=15 ->  1899 bytes   OVER BUDGET
```

Each additional process costs ~80 bytes worst-case. **Eight is the maximum**
that preserves the guarantee.

A packet that fits on an idle Mac and fragments under load is precisely the bug
the budget test exists to prevent, so the worst case governs.

**Decision: raise `TOP_PROC_COUNT` from 5 to 8.** Rejected alternatives:

- **A second packet type for processes at 1 Hz.** This is what ANALYSIS.md §2
  already predicts ("a future top-30 view would need a second packet type at a
  slower cadence, not a different transport"). Correct, and the right move *if*
  a long process list is ever wanted — but it adds a second decode path and
  forces the UI to hold two independently-arriving streams, which is real
  complexity for a list nobody reads at arm's length on a 7" panel. Deferred,
  not dismissed.
- **Trimming the wire format** (dropping `pid`, sending memory in MB, short
  keys) buys 10-14 processes but trades away the JSON inspectability
  ANALYSIS.md §3 chose deliberately over a 40% saving. Rejected.

---

## 3. Layout: give height to what scales with height

Eight processes in a 23-row panel still fills only 40% of it. Stretching a
fixed-length list to fill a tall panel just moves the blank space inside the
border.

**The rule: vertical space goes to content that absorbs it — history
sparklines, which get more useful with every row — while fixed-row content
(process list, temperature readout) is sized to its actual content.**

Applying that rule, measured on a mock at exactly 128×37:

| Layout | Blank |
|---|---|
| Today | 73.9% |
| Stretch existing panels | 63% |
| Denser panels, more fields | 56% |
| **Height to sparklines, lists sized to content** | **51%** |

The residual 51% is mostly the area *above* sparkline data — a graph reading
low, which is information, not waste.

### Target arrangement (128×37)

```
 ● macAbre.local   up 22h 47m                        14:32:07    Thu 17 Sep
┌ CPU ──────────────────────────────────┐┌ MEMORY ─────────────────────┐
│ 14.9%   usr 3.9%   sys 11.0%  nice 0.0││11.4G / 16.0G  71%  press 47%│
│ ████······························    ││███████████████··············│
│ load 2.06 2.71 3.04   10 cores ▆▅▄▃▂  ││compressed 5.5G  swap 11.8/13.9
│ history 4 min                         ││<memory history sparkline>   │
│ <CPU history — takes all spare rows>  │└─────────────────────────────┘
│                                       │┌ THERMAL ────────────────────┐
│                                       ││ 42.6°C SoC  48.1°C GPU  33.0°C
│                                       ││<temperature history>        │
└───────────────────────────────────────┘└─────────────────────────────┘
┌ TOP PROCESSES (8, sized to content) ──┐┌ NETWORK ────────────────────┐
│   %CPU      MEM   PROCESS             ││▼ 3.4K/s  ▲ 10.3K/s   en0    │
│  14.2%     118M   com.apple.WebKit... ││<rx graph, capped 4 rows>    │
│   ... 8 rows ...                      ││<tx graph, capped 4 rows>    │
└───────────────────────────────────────┘│session ▼ 2.02G  ▲ 1.78G     │
 LIVE  en0  18 pkts                      └─────────────────────────────┘
```

### Content added, all from fields already on the wire

| Panel | Added | Source field |
|---|---|---|
| CPU | load averages 1/5/15, core count, nice% | `cpu.load`, `cpu.ncpu`, `cpu.nice` |
| MEMORY | compressed, swap denominator, used-% figure | `mem.compressed`, `mem.swap_total` |
| MEMORY | usage history sparkline | derived from existing samples |
| THERMAL | temperature history sparkline | derived from existing samples |
| NETWORK | session totals | `net.rx_total`, `net.tx_total` |
| header | clock and date | `ts` (see §4) |

Two new `Ring` histories (memory, temperature) join the three that exist. At
`HISTORY = 120` samples each, that is ~2 KB more RAM on a 512 MB Pi.

The network sparkline height cap stays. The reclaimed rows go to other panels.

### Responsive behaviour

The existing `NARROW`/`SHORT` thresholds and stacking behaviour are unchanged.
New panels and sparklines are added only where height allows, following the
existing `if rows[n].height > 0` guard pattern, so the 80×24 SSH view and the
degenerate sizes in the layout tests keep working.

---

## 4. Clock

The Pi Zero has **no RTC**: its clock is whatever NTP gave it at boot, or
wrong, with no indication which. A clock that lies is worse than no clock on an
always-on panel.

**The clock reads from the Mac's `ts` field**, already present in every packet
at 5 Hz. The Mac's clock is NTP-synced and authoritative. This costs zero bytes
and zero CPU.

When no packet has ever arrived, or presence is `Asleep`, the clock renders
dimmed placeholder (`--:--:--`) rather than a stale or fabricated time,
consistent with how the dashboard already treats absent temperature
(`unavailable`) and absent network (`no interface`).

Timestamps are rendered in **UTC**, not local time. The packet carries Unix
milliseconds with no timezone, the Pi has no reliable zone database guarantee,
and a silently-wrong local time is the failure this section exists to avoid. If
local time is wanted later, the Mac should send its UTC offset explicitly —
that is a protocol change, deferred.

Smooth sub-second ticking between packets is **not** implemented: packets
arrive every 200 ms, which is already twice the 100 ms frame rate, so the
displayed second advances correctly without interpolation.

---

## 5. Testing

Following the existing pattern in `crates/tui/tests/layout.rs`, which renders
to `TestBackend` at every plausible size and asserts no panic and no overflow —
a layout panic on a headless Pi means a power cycle.

| Test | Protects |
|---|---|
| Worst-case packet at `TOP_PROC_COUNT = 8` | The single-datagram guarantee (existing test, new constant) |
| Render at 128×37 with all new panels | The target panel |
| Render at every size in the existing matrix | No panic/overflow from new widgets |
| Clock with no packet / stale / asleep | Never displays a fabricated time |
| Clock formatting across epoch values | Correct date rollover |
| Blank-cell ratio at 128×37 below a threshold | The density goal itself, as a regression guard |
| Agent emits 8 processes | Collection honours the new count |

The blank-ratio test is the one that keeps this work from silently regressing;
it asserts a ceiling (e.g. ≤60%) rather than an exact figure, so ordinary
layout tweaks do not make it brittle.

---

## 6. Work plan

1. `proto`: `TOP_PROC_COUNT` 5 → 8; confirm the budget test still passes.
2. `agent`: nothing — `procs.rs` already truncates to the constant.
3. `tui/state.rs`: add `mem_history` and `temp_history` rings.
4. `tui/ui/widgets.rs`: extend CPU, MEMORY, THERMAL, NETWORK panels.
5. `tui/ui/mod.rs`: re-proportion the layout per §3; add the header clock.
6. `tui/ui/format.rs`: clock and date formatting from Unix ms.
7. Tests per §5.
8. Update README's sample frame and "What it shows"; update ANALYSIS.md §2 with
   the measured worst-case ceiling.

## 7. Risks

**The 8-process cap may still feel thin.** It is a genuine improvement over 5
but not a process *browser*. If it disappoints, §2 records the second-packet
design as the next step rather than a rewrite.

**Render cost on ARMv6 grows** with two more sparklines. The Pi's budget has
never been measured as tightly as the Mac's; if the frame time becomes visible,
sparkline data can be decimated to the panel width before rendering, which it
largely already is via `Ring::tail(width)`.
