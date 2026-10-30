# AGENTS.md

spectra is a terminal spectrum analyzer in Rust. It captures system audio
through native PipeWire and renders it with ratatui. `README.md` covers the
user-facing features, keys and config options. This file covers working on
the code.

## Commands

```sh
cargo build --release                # binary at target/release/spectra
cargo test                           # unit tests, no audio needed
cargo clippy --all-targets           # must stay warning-free
./target/release/spectra --list-devices
```

Build needs the PipeWire and D-Bus headers and clang.

**Don't run `cargo fmt` on the whole project.** The code is formatted by
hand in places (the `STYLES` table in `src/viz/mod.rs`, multi-argument
calls), and rustfmt rewrites them. Format only the lines you add, in the
style of the code around them.

## Version control

The repo is a colocated jj (Jujutsu) repo. Git always reports a detached
HEAD. That is normal, so don't create git branches or warn about it.

- Commit with `jj commit -m "..."`, not `git commit`.
- Messages follow `type: summary` (`feat`, `fix`, `refactor`, `test`), with
  a body explaining why when the change isn't obvious.
- Use `jj diff --stat --no-pager` and `jj log --no-pager` to inspect state.

## Layout

| Path | Role |
| --- | --- |
| `src/main.rs` | CLI (clap), config load, resolving the capture target |
| `src/app.rs` | `App` state, main loop, key handling, device switching |
| `src/ui.rs` | Header, footer, popups, the `KEYS` binding table |
| `src/audio.rs` | PipeWire capture thread, device listing, link tracking |
| `src/dsp.rs` | `Analyzer` (FFT to log bands, smoothing, peak caps), `Agc`, `BandAgg` |
| `src/config.rs` | `Config`, `sanitize()`, the `ConfigKey` trait, unit tests |
| `src/theme.rs` | `ThemeId`, palettes, gradient lookup |
| `src/nowplaying.rs` | Background MPRIS poller for the now-playing line |
| `src/viz/mod.rs` | `Visualizer` trait, `STYLES` registry, shared drawing helpers |
| `src/viz/*.rs` | One file per visualization style |

## Threads

1. **PipeWire realtime thread.** It runs the stream's `process` callback.
   No locks and no allocation here. Frames go into a lock-free SPSC ring
   (`ringbuf`), and counters are atomics.
2. **Capture main-loop thread** (`pw-capture`). It runs PipeWire's main
   loop with the registry and `state_changed` callbacks. They share state
   through `Rc<RefCell<Graph>>`, and the result reaches the UI through
   `Capture::source` (an `Arc<Mutex<_>>`).
3. **UI thread.** It runs at about 60 fps (`FRAME` in `app.rs`): it drains
   the ring, runs the shared feature analyzer, updates the active style and
   draws.
4. **MPRIS thread.** It polls D-Bus once per second. D-Bus calls can stall,
   so they must never run on the UI thread.

## How to extend

**A new visualization style.**
1. Add `src/viz/<name>.rs` and implement `Visualizer` (`update` and `render`).
   `render` must handle any area size, including tiny ones.
2. Add one entry to `STYLES` in `src/viz/mod.rs`. Cycling, the style menu,
   config persistence and `--style` validation all come from that table.
3. If it has tunables, add a `[name]` section struct in `config.rs`, clamp
   it in `sanitize()`, and list it in the README's configuration section.

**Spectrum-based styles.** Build the analyzer with
`Analyzer::new(cfg, n_bars)` and call `analyzer.sync(cfg)` each frame to
pick up live gain and smoothing changes. Use `.with_fixed_smoothing(0)` if
the style must ignore the smoothing setting. Styles that only need coarse
levels should read `frame.features` instead of running their own analyzer.

**Shared helpers in `viz/mod.rs`.** Check these before writing drawing or
level code: `put_hspan`, `put_half_block`, `draw_bar` with `BarColumn`,
`bar_layout`, `BrailleCanvas`, `OnsetDetector`, `PeakScaler`, `Rng`. For
dB-based auto gain, use `dsp::Agc`.

**A new key binding.** Handle it in `App::on_key`, add a row to `KEYS` in
`ui.rs` (which drives both the footer and the help popup), and update the
key table in `README.md`.

**A new string-valued config option.** Use an enum that implements
`ConfigKey` and mark the field `#[serde(with = "key_serde")]`. Don't use a
plain serde enum. `Config::load` falls back to defaults for the whole file
when parsing fails, so one bad value would wipe every setting. `key_serde`
parses case-insensitively, accepts aliases, and resets only the bad field.
Numeric options get clamped in `sanitize()`.

## Dependencies

`pipewire` comes from pipewire-rs git (0.10), not crates.io. The crates.io
0.8 crates fail to build with clang 22 and later, because their bindgen
output has opaque structs. Keep the git dependency. The 0.10 API uses
`MainLoopRc`, `ContextRc`, `connect_rc` and `StreamBox`.
`audio::connect()` wraps the setup.

## PipeWire behavior to know

- A stream whose `target.object` matches no node still connects without an
  error. WirePlumber links it to the default output instead. That's why
  `main.rs` checks the saved device against the device list before starting
  capture.
- When a linked device disappears, WirePlumber moves the stream to the
  default output, and it moves the stream back when the device returns. The
  capture thread reports the real source, and `App::check_linked_source`
  updates the header from it.
- Sink monitors show up as links from the sink node itself.

## Testing against real audio

`cargo test` covers DSP, config and drawing helpers. For behavior that
needs PipeWire:

- Point `XDG_CONFIG_HOME` at a scratch directory with its own
  `spectra/config.toml`. Never read or overwrite the user's real
  `~/.config/spectra/config.toml`.
- The app needs a terminal. Run it under a pseudo-terminal:
  `script -qfc "stty cols 140 rows 30; exec ./target/release/spectra" out.log`.
- The user often has their own spectra running. Start yours as
  `./target/release/spectra` and stop it by that exact command line
  (`pgrep -f '^\./target/release/spectra$'`). Never use a broad
  `pkill spectra`.
- Inspect links with `pw-dump` (match the node by `target.object` or id)
  or `pw-link -l`.
- To simulate unplugging a device, create a temporary null sink with
  `pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=spectra-test-sink media.class=Audio/Sink object.linger=true audio.position=[FL FR] priority.session=0 priority.driver=0 }'`,
  then remove it with `pw-cli destroy <id>`. The low priorities keep it
  from becoming the default output. Check afterwards that no test sinks are
  left and that the default sink (`wpctl inspect @DEFAULT_AUDIO_SINK@`) is
  unchanged.
- ratatui redraws only the cells that changed, so a screen capture doesn't
  hold full frames. Search the captured output for specific text, such as a
  status message, instead of reading the "last" header line.
