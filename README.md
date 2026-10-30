# spectra

A retro hi-fi spectrum analyzer for the terminal. Captures whatever is
currently playing on your system via native PipeWire and renders classic
bars with falling peak-hold caps, log-spaced from 30 Hz to 16 kHz.

```
                    ▂
        ▄     ▁    █
   ▂   █ █   █    █ ▄
  ▄█ ▄ █ █ ▂ █ ▄  █ █ ▁
 ▆██ █ █ █ █ █ █ ▄█ █ █
 ███▆█▆█▆█▆█▆█▆█▆██▆█▆█
  50  200  1k  4k  16k
```

## Requirements

- PipeWire (with `libpipewire-0.3` headers) and clang at build time
- D-Bus (`dbus-1` headers) for the MPRIS now-playing line
- A terminal with Unicode block characters; true color for the RGB themes

## Build & run

```sh
cargo build --release
./target/release/spectra
```

By default it captures the monitor of your default audio output. Options:

```
-d, --device <QUERY>   capture a specific device (matches name/description)
-l, --list-devices     list sinks (monitors) and sources, then exit
-t, --theme <THEME>    terminal | classic | catppuccin | gruvbox | nord | tokyo-night
-b, --bars <N>         number of frequency bars (8–96)
-s, --style <STYLE>    see the style list below
```

## Visualization styles

Cycle forward with `v`, backward with `V`:

- **bars** — classic hi-fi spectrum: vertical bars, falling peak-hold caps,
  frequency axis.
- **mirror** — stereo deck view: the left channel grows up from a water
  line, the right channel grows down, each with its own peaks.
- **ripples** — rain on a lake: bass hits spawn large ripples near the
  center, treble activity drizzles small ones; expanding wave packets
  interfere, crests take the hot end of the theme gradient, troughs the cool
  end.
- **fractal** — a morphing Julia set: bass drives the constant along the
  Mandelbrot cardioid boundary, beats shatter the set into dendrites, level
  breathes the zoom, and treble cycles the palette.
- **spectrogram** — scrolling waterfall: frequency left→right, time flowing
  upward, intensity as color; two time slices per row via half-blocks.
- **scope** — oscilloscope: the raw waveform as a braille trace with a
  rising-edge trigger; splits into stereo dual-trace when the window is tall.
- **vector** — vectorscope / goniometer with phosphor persistence: mono
  collapses to the vertical M axis, wide stereo blooms into a cloud.
- **vu** — two analog VU meters: braille needle on an arc scale with spring
  ballistics, dB readout, and latching peak LEDs.
- **fire** — demoscene fire stoked by the spectrum: bass roars on the left,
  treble sparks on the right; dies down to embers in silence.
- **particles** — starfield: beats fire radial bursts from the center,
  treble drizzles slow drifters.
- **matrix** — matrix rain where each column is a frequency band: its
  energy drives the fall speed, glyph churn, and glow.
- **radial** — the spectrum wrapped around a pulsing iris, bass at the top
  sweeping symmetrically down to treble.
- **skyline** — strip-chart of the loudness envelope scrolling right to
  left, with a bright peak ridge above the RMS silhouette.

New styles implement the `Visualizer` trait in `src/viz/` and add one entry
to the `viz::STYLES` registry (id, display name, constructor) — everything
else (cycling, the style menu, config persistence, CLI validation) derives
from that table.

## Keys

| Key     | Action                                    |
| ------- | ----------------------------------------- |
| `v` `V` | next / previous visualization style       |
| `m`     | visualization style menu                  |
| `↑` `↓` | input gain up / down                      |
| `+` `-` | more / fewer bars                         |
| `s`     | cycle smoothing (fast / medium / smooth)  |
| `p`     | toggle peak-hold caps                     |
| `a`     | toggle auto gain (AGC)                    |
| `t`     | cycle color theme                         |
| `d`     | pick capture device                       |
| `f`     | hide/show header & footer (focus mode)    |
| `?` `h` | help                                      |
| `q` `Esc` | quit (settings are saved)               |

## Themes

`terminal` (default) maps the classic green/yellow/red layout onto your
terminal's ANSI palette so it follows whatever theme the terminal uses.
`classic` is the authentic true-color hi-fi gradient; `catppuccin` (Mocha),
`gruvbox`, `nord`, and `tokyo-night` match the popular color schemes.

## Now playing

If an MPRIS-capable player (Spotify, mpv, browsers, …) is running, the
current artist, title, and album are shown on a line above the controls —
it stays visible even in focus mode. The line disappears when nothing is
playing or paused.

## Configuration

Settings (theme, bars, gain, smoothing, device, …) are saved to
`~/.config/spectra/config.toml` on exit and restored on the next start.
`fft_size`, `freq_min`, and `freq_max` can be tuned there as well.

Additional config-file-only tunables:

- `band_agg` — how FFT bins fold into a bar for bars/mirror: `"energy"`
  (default), `"max"` (punchy), `"avg"` (smooth), `"hybrid"`.
- `[mirror] mode` — `"lr"` (left/right) or `"ms"` (mid/side stereo width).
- `[scope] window_ms` — oscilloscope time window (5–200 ms).
- `[fire] cooling` — flame-height multiplier; >1 shorter, <1 taller.
- `[particles] density` — spawn-rate multiplier (0.1–4).
- `[radial] spokes` — spokes around the circle (24–256).
- `[spectrogram] slice_ms` / `floor` / `fade` — scroll speed, display
  threshold, and fading of older rows.

The vectorscope also shows an L/R correlation meter along its bottom edge
(+1 mono, 0 wide, −1 out of phase).

## How it works

A dedicated thread runs a PipeWire capture stream (`stream.capture.sink`)
that pushes stereo float samples into a ring buffer. The UI thread runs
at ~60 fps: it takes the latest 2048-sample window, applies a Hann window
and a real FFT, folds the magnitudes into log-spaced bands, converts them to
dB over a 64 dB display range, and animates bars (asymmetric attack/decay
smoothing) plus gravity-driven peak caps. Auto gain tracks the loudest
recent band and slides the display ceiling so any source fills the meter.
