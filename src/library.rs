//! The list of installed games (library.json).

use std::fs;

use anyhow::{Context, Result};

use crate::models::InstalledGame;
use crate::paths::Paths;

pub struct Library {
    pub games: Vec<InstalledGame>,
}

impl Library {
    pub fn load(paths: &Paths) -> Result<Self> {
        let games = match fs::read_to_string(&paths.library_file) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", paths.library_file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).context("could not read library file"),
        };
        Ok(Self { games })
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        if let Some(parent) = paths.library_file.parent() {
            fs::create_dir_all(parent)?;
        }
        // Write to a temp file and rename so a crash mid-write can't leave a
        // half-written library.json behind.
        let tmp = paths.library_file.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&self.games)?)?;
        fs::rename(&tmp, &paths.library_file)?;
        Ok(())
    }

    pub fn find(&self, game_id: u64) -> Option<&InstalledGame> {
        self.games.iter().find(|g| g.game_id == game_id)
    }

    pub fn find_mut(&mut self, game_id: u64) -> Option<&mut InstalledGame> {
        self.games.iter_mut().find(|g| g.game_id == game_id)
    }

    /// Insert, or replace the existing entry for the same game.
    pub fn upsert(&mut self, game: InstalledGame) {
        self.games.retain(|g| g.game_id != game.game_id);
        self.games.push(game);
        self.games.sort_by_key(|g| g.title.to_lowercase());
    }

    pub fn remove(&mut self, game_id: u64) -> Option<InstalledGame> {
        let idx = self.games.iter().position(|g| g.game_id == game_id)?;
        Some(self.games.remove(idx))
    }
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Time played, e.g. "45m" or "3h 12m".
pub fn format_playtime(secs: u64) -> String {
    let minutes = secs / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "under a minute".to_owned(),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// How long ago a timestamp was, e.g. "5 min ago" or "yesterday".
pub fn format_ago(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => plural(secs / 3_600, "hour"),
        86_400..172_800 => "yesterday".to_owned(),
        172_800..2_592_000 => plural(secs / 86_400, "day"),
        2_592_000..31_536_000 => plural(secs / 2_592_000, "month"),
        _ => plural(secs / 31_536_000, "year"),
    }
}

#[cfg(test)]
mod tests {
    use super::{format_ago, format_playtime};

    #[test]
    fn formats_playtime() {
        assert_eq!(format_playtime(30), "under a minute");
        assert_eq!(format_playtime(45 * 60), "45m");
        assert_eq!(format_playtime(2 * 3600), "2h");
        assert_eq!(format_playtime(3 * 3600 + 12 * 60 + 59), "3h 12m");
    }

    #[test]
    fn formats_time_ago() {
        let now = 1_000_000_000;
        assert_eq!(format_ago(now - 10, now), "just now");
        assert_eq!(format_ago(now - 300, now), "5 min ago");
        assert_eq!(format_ago(now - 3_600, now), "1 hour ago");
        assert_eq!(format_ago(now - 90_000, now), "yesterday");
        assert_eq!(format_ago(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(format_ago(now - 65 * 86_400, now), "2 months ago");
        assert_eq!(format_ago(now - 800 * 86_400, now), "2 years ago");
        assert_eq!(format_ago(now + 50, now), "just now");
    }
}
