# Pixel Dashboard Design

**Date:** 2026-10-07
**Status:** approved in conversation; awaiting review of this written spec

## Goal

Make the Pi's panel look like the Mac it sits under. Today the dashboard is a
text-console program: 16 colours, a 1-bit 8x16 font and square box-drawing
borders. A theme cannot get past that ceiling (the `graphite` theme proved it
and was judged "really underwhelming"). This replaces the Pi's output with a
pixel renderer that draws a macOS-style dashboard straight to the framebuffer:
anti-aliased type, rounded cards with shadows, smooth charts and ring gauges.

Success: the panel reads as a macOS widget sitting under a macOS desktop, shows
everything the text dashboard shows today, and costs the Pi Zero no more CPU
than the text dashboard does now.

## What the user asked for

- Everything today's dashboard shows, redesigned. Nothing dropped.
- Pixels by default on the Pi; a `--text` flag brings the console version
  back. SSH and Mac terminals keep the text version.
- One new look designed from scratch: "really mac style", greyish
  backgrounds, in line with the rest of the desk. **No** red/yellow/green
  window-control dots. The existing text themes are not carried over.
- Charts move once a second; numbers and meters glide.
- The approved mockup is the reference (`2026-10-07-pixel-dashboard-mockup.png` beside this spec).

## Evidence it is feasible

A throwaway program measured on the Pi Zero W (1024x600 RGB565, rotated 180
degrees), building for `arm-unknown-linux-musleabihf`:

| Version | Draw | Pack to RGB565 | Write |
|---|---|---|---|
| floats, no caching | 341 ms | 121 ms | — |
| glyph cache, static layer | 122 ms | 128 ms | — |
| u8 pixels, integer blending | **70 ms** | **30 ms** | 4 ms |

About 10% of the single core at 1 Hz; the text dashboard uses 13-19%. No
tearing was seen in the spike's own output. The richer mockup costs more to
draw (shadows, rings), which the static layer absorbs: it is drawn once.

## Architecture

The data side is untouched. `receiver`, `state::Dashboard`, `beacon`,
`brightness`, `hdmi` and the presence tracker are shared by both renderers.
A new module renders a `Dashboard` into pixels; `main.rs` picks a renderer at
startup.

```
receiver ─► Dashboard ─┬─► ui::render (ratatui, text)   ── terminal
                       └─► pixel::render (new)          ── /dev/fb0
```

### Units

Each is a file under `crates/tui/src/pixel/`, testable on the Mac without a
Pi.

- **`canvas.rs` — `Canvas`.** A `Vec<[u8; 3]>` RGB buffer with integer alpha
  blending. Primitives: fill, rounded rectangle (anti-aliased signed
  distance), soft shadow, top-edge highlight, ring arc with round caps, area
  chart (gradient fill plus a 2 px anti-aliased line), dashed horizontal
  line. No floats in the per-pixel inner loops except where a primitive's
  geometry needs them (distance). Depends on nothing.
- **`text.rs` — `Typeface`.** Wraps `fontdue` with a glyph cache keyed by
  (face, char, size). Measures and draws strings, left- or right-aligned.
  The four Inter faces are embedded with `include_bytes!`, so the binary
  has no runtime font files.
- **`layout.rs`.** Pure functions from screen size to card rectangles and
  element positions. The landscape 1024x600 layout matches the mockup.
  Portrait (600x1024, `--portrait` installs) stacks the same cards in one
  column. No drawing here, so it is unit-testable.
- **`scene.rs`.** Draws each card from a `Dashboard`: the static layer
  (background, shadows, card bodies, highlights, labels, grid lines) and
  the dynamic layer (values, bars, rings, charts). Each card is a function
  `fn cpu(c: &mut Canvas, t: &Typefaces, r: Rect, dash: &Dashboard, now)`.
- **`framebuffer.rs` — `Framebuffer`.** Opens `/dev/fb0`, reads geometry,
  bit depth and stride from the `FBIOGET_VSCREENINFO` and
  `FBIOGET_FSCREENINFO` ioctls, reads `/sys/class/graphics/fbcon/rotate`,
  and packs the canvas into RGB565 (or XRGB8888 if the panel reports 32
  bpp) with the rotation applied. Writes the whole frame with one `pwrite`.
- **`console.rs`.** Takes tty1 out of text mode with `KDSETMODE KD_GRAPHICS`
  so the kernel's console stops drawing over the frame (cursor, boot
  messages, the periodic repaint the text version needed), and restores
  `KD_TEXT` on exit, including on panic and SIGTERM.

### Frame loop

The existing loop structure stays: take the newest packet, update presence,
backlight and HDMI, render, then poll stdin for keys until the frame budget
is spent.

- The static layer is drawn once into a cached canvas. It is redrawn only
  when the screen size, the host name or the dimmed state changes.
- Each frame copies the static layer, draws the dynamic layer, packs and
  writes.
- **Cadence.** The frame is rendered once a second (charts scroll at 1 Hz,
  as now), plus extra frames while a value is easing: at most 5 per
  second, and only for 400 ms after a packet (`EASE_MS`). When nothing
  changed, nothing is written.
- **Tearing.** One `pwrite` of the full frame. The panel already showed no
  tearing in the spike at that size. If tearing appears with the full
  design, the fallback is to write only the rows that changed.

### Mode selection

- `--text` forces the text renderer.
- Otherwise the pixel renderer runs when `/dev/fb0` opens for writing and
  `TERM=linux`; anything else (SSH, the Mac's terminal) gets text.
- If the pixel renderer fails at startup, it logs why and falls back to
  text, so a bad framebuffer never leaves the panel blank.

### Keys

stdin stays in raw mode in pixel mode. `q`, `Esc` and Ctrl-C quit; `t` is
ignored because there is one pixel look.

## Visual design

Reference: the mockup PNG. Values below are the mockup's.

**Desktop.** Vertical gradient `#232326` to `#161618`, with a faint warm glow
in the upper right (`#3a2a33` at 35%), like a dimmed wallpaper.

**Menu strip** (30 px). Black at 28% over the desktop. Left: host name
(Inter SemiBold 14) and uptime (Regular 13, secondary). Right: date and time
(Medium 13). Centre: a presence capsule, a 6 px dot plus "Live · en0", tinted
to the state: green live, orange stale, grey asleep or display off, blue
waiting.

**Cards.** Radius 18, fill `#2a2a2d` at 92%, a soft 14 px shadow offset 4 px
down, and a 1 px white highlight at 10% that fades out down the top 60% of the
edge. Card titles in Inter SemiBold 13, secondary colour.

**Colours** (macOS dark appearance).

| Role | Value |
|---|---|
| Primary label | `#f5f5f7` |
| Secondary label | `#98989d` |
| Tertiary label | `#636366` |
| Separator | `#3a3a3c` |
| Well (meter tracks) | `#1f1f22` |
| Accent / CPU / download | `#5293e1` |
| System CPU | `#64d2ff` |
| Good (memory ring, pressure) | `#30d158` |
| Busy (warning) | `#ff9f0a` |
| Critical | `#ff453a` |
| Upload | `#bf5af2` |

Severity follows the existing thresholds (`theme::scale`, `temp_scale`):
normal uses the card's own colour, warning turns orange, critical red.

**Type.** Inter (OFL 1.1), embedded: Regular, Medium, SemiBold, and Display
SemiBold for big numbers. The licence file ships beside the fonts.

**Cards and contents** (landscape, 1024x600):

- **CPU** (16,44 624x316). Total as a big number (Display 64) with a smaller
  `%`. Under it, User, System and Load in caption/value pairs. Top right,
  ten core capsules (14x86, radius 7) that fill from the bottom: blue,
  orange at 60%, red at 85%. Bottom, the history chart: total (blue) over
  system (teal), smooth area, three dashed grid lines, a "4 min" caption.
- **Memory** (656,44 352x150). Ring gauge (radius 40, 10 px thick) with the
  used percentage in its centre. Beside it, used GB (Display 26) "of 16 GB",
  then Pressure, Compressed and Swap in caption/value pairs. Swap turns
  orange when in use.
- **Temperature** (656,210 168x150). SoC as a big number with a degree sign,
  battery as a caption, a small orange trend line. Shown as "—" when the
  Mac does not report temperature.
- **Network** (840,210 168x150). Download and upload rates with arrows and
  units, over a small dual trend chart (blue down, purple up).
- **Top processes** (16,376 992x208). Up to 8 rows (the protocol ceiling) at
  26 px, in alternating shading. Each row: a 16 px rounded square in a
  colour hashed from the name, with the initial; the name; a thin CPU bar;
  CPU percentage; memory. Rows that do not fit are dropped from the bottom.

**States.**

- **Waiting** (no packet yet): the desktop and menu strip, with one centred
  card: "Waiting for a Mac", and the existing hint text.
- **Stale or asleep:** every card dims to 45% and the capsule changes
  colour. HDMI power-off is unchanged.

**Motion.** The big numbers, core capsules, meters and ring ease over 400 ms
using the existing `Eased`. Charts step once a second.

## Deployment

- `install-pi.sh` gains `--text` to install the console version. Without
  it, the service runs pixels. `--theme` and `--font` keep working for text
  mode.
- The service unit is unchanged: it already owns tty1 and runs as root, so
  `/dev/fb0` and `KDSETMODE` are available.
- On exit (stop, crash, restart) the console returns to text mode, so a
  `getty` or the text renderer can take over.
- The 180-degree flipped panel works, because rotation is read from
  `fbcon/rotate` and applied by the renderer.

## Error handling

- No panics in the render path. Every primitive clips to the canvas; empty
  rectangles draw nothing.
- Framebuffer open, ioctl or format failures fall back to text at startup.
  A write failure mid-run is logged once and retried on the next frame.
- Missing data (no temperature, no network, no processes) renders as "—" or
  an empty card, never as zero.

## Testing

- **Unit tests** (run on the Mac): canvas primitives (coverage at edges,
  clipping, alpha blending), RGB565 packing and each rotation, layout
  rectangles for landscape and portrait (non-overlapping, inside the
  screen), text measurement, and process-row truncation.
- **Golden image.** An example renders a fixed `Dashboard` to PNG on the Mac,
  like the existing `snapshot` example does for text. It is used to check
  the design by eye.
- **On the Pi.** Measure CPU with `top` over a minute: at most the text
  dashboard's 13-19%. Capture `/dev/fb0` frames to check for tearing and
  for anything the console still draws over the frame.

## Out of scope

- Real app icons: the agent does not send them, and adding them would break
  the 1400-byte packet budget.
- A light appearance, other themes in pixel mode, or a theme switcher.
- Touch input.
- Any change to the Mac agent or the protocol.
