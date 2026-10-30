mod app;
mod audio;
mod config;
mod dsp;
mod nowplaying;
mod theme;
mod ui;
mod viz;

use anyhow::{bail, Result};
use clap::Parser;

use audio::CaptureTarget;
use config::{Config, ConfigKey};
use theme::ThemeId;

/// Retro hi-fi spectrum analyzer for the terminal, powered by PipeWire.
#[derive(Parser)]
#[command(name = "spectra", version, about)]
struct Cli {
    /// Capture from a specific device (match by node name or description,
    /// see --list-devices). Default: the default output's monitor.
    #[arg(short, long)]
    device: Option<String>,

    /// List available capture devices and exit.
    #[arg(short, long)]
    list_devices: bool,

    /// Color theme: terminal, classic, catppuccin, gruvbox, nord, tokyo-night.
    #[arg(short, long)]
    theme: Option<String>,

    /// Number of frequency bars.
    #[arg(short, long)]
    bars: Option<usize>,

    /// Visualization style.
    #[arg(short = 's', long, value_parser = clap::builder::PossibleValuesParser::new(viz::style_ids()))]
    style: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.list_devices {
        let devices = audio::enumerate_devices()?;
        println!("Available capture devices (default: default output monitor):\n");
        for d in &devices {
            let kind = if d.is_sink { "monitor" } else { "input  " };
            println!("  [{kind}] {}\n            node: {}", d.description, d.name);
        }
        return Ok(());
    }

    let mut cfg = Config::load();
    cfg.sanitize();
    if let Some(t) = &cli.theme {
        match ThemeId::from_key(t) {
            Some(id) => cfg.theme = id,
            None => bail!(
                "unknown theme '{t}' (expected one of: terminal, classic, catppuccin, gruvbox, nord, tokyo-night)"
            ),
        }
    }
    if let Some(b) = cli.bars {
        cfg.bars = b.clamp(8, 96);
    }
    if let Some(s) = cli.style {
        cfg.visualizer = s; // clap already checked it against viz::STYLES
    }

    // Resolve the capture target: CLI flag beats config, both fall back to
    // the default output monitor if the device can't be found.
    let mut target = CaptureTarget::default();
    let mut label = "Default output".to_string();
    if let Some(query) = &cli.device {
        let devices = audio::enumerate_devices()?;
        let q = query.to_lowercase();
        match devices.iter().find(|d| {
            d.name == *query
                || d.name.to_lowercase().contains(&q)
                || d.description.to_lowercase().contains(&q)
        }) {
            Some(d) => {
                target = CaptureTarget {
                    node_name: Some(d.name.clone()),
                    is_sink: d.is_sink,
                };
                label = d.description.clone();
            }
            None => bail!("no device matching '{query}' (try --list-devices)"),
        }
    } else if let Some(name) = &cfg.device {
        target = CaptureTarget {
            node_name: Some(name.clone()),
            is_sink: cfg.device_is_sink,
        };
        label = name.clone();
        // Use the nicer description if the device is currently present.
        if let Ok(devices) = audio::enumerate_devices() {
            if let Some(d) = devices.iter().find(|d| &d.name == name) {
                label = d.description.clone();
            }
        }
    }

    let mut terminal = ratatui::init();
    let result = app::run(&mut terminal, cfg, target, label);
    ratatui::restore();

    let cfg = result?;
    if let Err(e) = cfg.save() {
        eprintln!("warning: could not save config: {e}");
    }
    Ok(())
}
