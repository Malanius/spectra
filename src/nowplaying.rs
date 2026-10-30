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
    let _ = thread::Builder::new().name("mpris".into()).spawn(move || {
        // One D-Bus connection for the poller's lifetime; rebuilt only when
        // a lookup fails (e.g. the session bus went away).
        let mut finder = None;
        loop {
            if finder.is_none() {
                finder = PlayerFinder::new().ok();
            }
            let info = match finder.as_ref().map(poll) {
                Some(Ok(info)) => info,
                Some(Err(())) => {
                    finder = None;
                    None
                }
                None => None,
            };
            *out.lock().unwrap() = info;
            thread::sleep(Duration::from_millis(1000));
        }
    });
    shared
}

/// `Err` means the player lookup itself failed and the finder should be
/// rebuilt; `Ok(None)` just means nothing is playing.
fn poll(finder: &PlayerFinder) -> Result<Option<NowPlaying>, ()> {
    let players = finder.find_all().map_err(|_| ())?;
    Ok(pick(players))
}

fn pick(players: Vec<mpris::Player>) -> Option<NowPlaying> {

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
