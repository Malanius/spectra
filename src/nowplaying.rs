use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mpris::{PlaybackStatus, PlayerFinder};

/// Metadata of the track currently playing on an MPRIS-capable player.
#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub playing: bool,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub player: String,
}

pub type Shared = Arc<Mutex<Option<NowPlaying>>>;

/// Start the background MPRIS poller. D-Bus round-trips can stall for
/// hundreds of milliseconds, so they must never run on the render thread.
/// The thread is detached; it dies with the process.
pub fn spawn_poller() -> Shared {
    let shared: Shared = Arc::new(Mutex::new(None));
    let out = shared.clone();
    let _ = thread::Builder::new().name("mpris".into()).spawn(move || loop {
        let info = poll();
        *out.lock().unwrap() = info;
        thread::sleep(Duration::from_millis(1000));
    });
    shared
}

fn poll() -> Option<NowPlaying> {
    let finder = PlayerFinder::new().ok()?;
    let players = finder.find_all().ok()?;

    // Prefer a player that is actually playing; fall back to a paused one.
    // Skip playerctld — it only mirrors another player and would show up
    // under its own name instead of the real application's.
    let mut best = None;
    for p in players {
        if p.identity().eq_ignore_ascii_case("playerctld") {
            continue;
        }
        let status = p.get_playback_status().ok();
        if matches!(status, Some(PlaybackStatus::Playing)) {
            best = Some((p, true));
            break;
        }
        if best.is_none() && matches!(status, Some(PlaybackStatus::Paused)) {
            best = Some((p, false));
        }
    }
    let (player, playing) = best?;

    let md = player.get_metadata().ok()?;
    let title = md.title().unwrap_or_default().to_string();
    let artist = md.artists().map(|a| a.join(", ")).unwrap_or_default();
    let album = md.album_name().unwrap_or_default().to_string();
    if title.is_empty() && artist.is_empty() {
        return None;
    }
    Some(NowPlaying {
        playing,
        artist,
        title,
        album,
        player: player.identity().to_string(),
    })
}
