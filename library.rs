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
